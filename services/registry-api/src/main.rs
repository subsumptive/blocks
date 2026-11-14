use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use block_package::{extract_archive, PackageId, PackageManifest, MAX_ARCHIVE_BYTES};
use block_registry_core::{
    hash_token, has_scope, ApiErrorBody, ApiErrorDetail, CreateTokenRequest,
    CreatedTokenResponse, PackageSummary, PackageVersionDetail, PaginatedPackages, PublishResponse,
    UserProfile, SCOPE_PUBLISH, SCOPE_READ, SCOPE_YANK,
};
use block_resolver::IndexVersionEntry;
use chrono::{Duration, Utc};
use serde::Deserialize;
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    db: PgPool,
    storage_dir: PathBuf,
}

struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: ApiErrorDetail {
                code: self.code,
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/subsumptive_registry".into());
    let storage_dir = PathBuf::from(
        std::env::var("REGISTRY_STORAGE_DIR").unwrap_or_else(|_| "./data/archives".into()),
    );
    tokio::fs::create_dir_all(&storage_dir).await?;

    let db = PgPoolOptions::new()
        .max_connections(16)
        .connect(&database_url)
        .await?;

    let state = Arc::new(AppState { db, storage_dir });

    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/openapi.json", get(openapi_spec))
        .route("/api/v1/packages", get(list_packages))
        .route("/api/v1/packages/search", get(search_packages))
        .route("/api/v1/packages/{scope}/{name}", get(get_package))
        .route("/api/v1/packages/{scope}/{name}/versions", get(list_versions))
        .route("/api/v1/packages/{scope}/{name}/{version}", get(get_version))
        .route("/api/v1/packages/{scope}/{name}/{version}/download", get(download_version))
        .route("/api/v1/packages/publish", post(publish_package))
        .route("/api/v1/packages/{scope}/{name}/{version}/yank", post(yank_version))
        .route("/api/v1/me", get(get_me))
        .route("/api/v1/tokens", post(create_token))
        .route("/api/v1/tokens/{id}", delete(revoke_token))
        .route("/index/{scope}/{name}", get(sparse_index))
        .layer(RequestBodyLimitLayer::new(MAX_ARCHIVE_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = std::env::var("REGISTRY_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Subsumptive Registry API listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "subsumptive-registry" }))
}

async fn openapi_spec() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "openapi": "3.1.0",
        "info": { "title": "Subsumptive Registry API", "version": "1.0.0" },
        "paths": {
            "/api/v1/health": { "get": { "summary": "Health check" } },
            "/api/v1/packages": { "get": { "summary": "List packages" } },
            "/api/v1/packages/search": { "get": { "summary": "Search packages" } },
            "/api/v1/packages/{scope}/{name}": { "get": { "summary": "Get package" } },
            "/api/v1/packages/{scope}/{name}/versions": { "get": { "summary": "List versions" } },
            "/api/v1/packages/{scope}/{name}/{version}/download": { "get": { "summary": "Download archive" } },
            "/api/v1/packages/publish": { "post": { "summary": "Publish package" } },
            "/api/v1/packages/{scope}/{name}/{version}/yank": { "post": { "summary": "Yank version" } },
            "/api/v1/me": { "get": { "summary": "Current user" } },
            "/api/v1/tokens": { "post": { "summary": "Create API token" } },
            "/api/v1/tokens/{id}": { "delete": { "summary": "Revoke API token" } }
        }
    }))
}

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    required_scope: &str,
) -> Result<Uuid, ApiError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "missing bearer token"))?;

    let token_hash = hash_token(auth.trim());
    let row = sqlx::query(
        "SELECT user_id, scopes, expires_at, revoked_at FROM api_tokens WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?
    .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "INVALID_TOKEN", "invalid API token"))?;

    let revoked_at: Option<chrono::DateTime<Utc>> = row.get("revoked_at");
    if revoked_at.is_some() {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "TOKEN_REVOKED", "API token has been revoked"));
    }
    let expires_at: Option<chrono::DateTime<Utc>> = row.get("expires_at");
    if let Some(exp) = expires_at {
        if exp < Utc::now() {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED", "API token has expired"));
        }
    }
    let scopes: Vec<String> = row.get("scopes");
    if !has_scope(&scopes, required_scope) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "INSUFFICIENT_SCOPE",
            format!("token lacks required scope '{required_scope}'"),
        ));
    }
    Ok(row.get("user_id"))
}

async fn get_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<UserProfile>, ApiError> {
    let user_id = authenticate(&state, &headers, SCOPE_READ).await?;
    let row = sqlx::query(
        "SELECT id, username, display_name, avatar_url, created_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    Ok(Json(UserProfile {
        id: row.get("id"),
        username: row.get("username"),
        display_name: row.get("display_name"),
        avatar_url: row.get("avatar_url"),
        created_at: row.get("created_at"),
    }))
}

async fn create_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateTokenRequest>,
) -> Result<Json<CreatedTokenResponse>, ApiError> {
    let user_id = authenticate(&state, &headers, SCOPE_READ).await?;
    let raw_token = format!("sub_{}", Uuid::new_v4().simple());
    let token_hash = hash_token(&raw_token);
    let expires_at = req
        .expires_in_days
        .map(|d| Utc::now() + Duration::days(d));

    let row = sqlx::query(
        "INSERT INTO api_tokens (user_id, name, token_hash, scopes, expires_at) VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(user_id)
    .bind(&req.name)
    .bind(&token_hash)
    .bind(&req.scopes)
    .bind(expires_at)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    Ok(Json(CreatedTokenResponse {
        id: row.get("id"),
        name: req.name,
        token: raw_token,
        scopes: req.scopes,
        expires_at,
    }))
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state, &headers, SCOPE_READ).await?;
    sqlx::query("UPDATE api_tokens SET revoked_at = NOW() WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_packages(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<PaginatedPackages>, ApiError> {
    search_packages(State(state), Query(q)).await
}

async fn search_packages(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<PaginatedPackages>, ApiError> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;
    let pattern = format!("%{}%", q.q.unwrap_or_default());

    let rows = sqlx::query(
        r#"
        SELECT p.scope, p.name, p.description, p.license, p.deprecated, p.updated_at,
               (SELECT pv.version FROM package_versions pv WHERE pv.package_id = p.id AND pv.status = 'published' ORDER BY pv.created_at DESC LIMIT 1) AS latest_version,
               (SELECT COUNT(*) FROM package_download_events de JOIN package_versions pv2 ON de.package_version_id = pv2.id WHERE pv2.package_id = p.id) AS downloads
        FROM packages p
        WHERE p.name ILIKE $1 OR p.scope ILIKE $1 OR COALESCE(p.description, '') ILIKE $1
        ORDER BY p.updated_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(&pattern)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let items = rows
        .into_iter()
        .map(|r| PackageSummary {
            scope: r.get("scope"),
            name: r.get("name"),
            latest_version: r.get("latest_version"),
            description: r.get("description"),
            license: r.get("license"),
            downloads: r.get::<Option<i64>, _>("downloads").unwrap_or(0),
            deprecated: r.get("deprecated"),
            updated_at: r.get("updated_at"),
        })
        .collect();

    Ok(Json(PaginatedPackages {
        items,
        total: 0,
        page,
        per_page,
    }))
}

async fn get_package(
    State(state): State<Arc<AppState>>,
    Path((scope, name)): Path<(String, String)>,
) -> Result<Json<PackageSummary>, ApiError> {
    let r = sqlx::query(
        r#"
        SELECT p.scope, p.name, p.description, p.license, p.deprecated, p.updated_at,
               (SELECT pv.version FROM package_versions pv WHERE pv.package_id = p.id AND pv.status = 'published' ORDER BY pv.created_at DESC LIMIT 1) AS latest_version,
               (SELECT COUNT(*) FROM package_download_events de JOIN package_versions pv2 ON de.package_version_id = pv2.id WHERE pv2.package_id = p.id) AS downloads
        FROM packages p
        WHERE p.scope = $1 AND p.name = $2
        "#,
    )
    .bind(&scope)
    .bind(&name)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?
    .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "NOT_FOUND", "package not found"))?;

    Ok(Json(PackageSummary {
        scope: r.get("scope"),
        name: r.get("name"),
        latest_version: r.get("latest_version"),
        description: r.get("description"),
        license: r.get("license"),
        downloads: r.get::<Option<i64>, _>("downloads").unwrap_or(0),
        deprecated: r.get("deprecated"),
        updated_at: r.get("updated_at"),
    }))
}

async fn list_versions(
    State(state): State<Arc<AppState>>,
    Path((scope, name)): Path<(String, String)>,
) -> Result<Json<Vec<PackageVersionDetail>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT pv.version, pv.description, pv.license, pv.sha256, pv.size_bytes, pv.yanked, pv.created_at
        FROM package_versions pv
        JOIN packages p ON pv.package_id = p.id
        WHERE p.scope = $1 AND p.name = $2 AND pv.status = 'published'
        ORDER BY pv.created_at DESC
        "#,
    )
    .bind(&scope)
    .bind(&name)
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let out = rows
        .into_iter()
        .map(|r| PackageVersionDetail {
            scope: scope.clone(),
            name: name.clone(),
            version: r.get("version"),
            description: r.get("description"),
            license: r.get("license"),
            sha256: r.get("sha256"),
            size_bytes: r.get("size_bytes"),
            yanked: r.get("yanked"),
            downloads: 0,
            dependencies: vec![],
            created_at: r.get("created_at"),
        })
        .collect();
    Ok(Json(out))
}

async fn get_version(
    State(state): State<Arc<AppState>>,
    Path((scope, name, version)): Path<(String, String, String)>,
) -> Result<Json<PackageVersionDetail>, ApiError> {
    let r = sqlx::query(
        r#"
        SELECT pv.version, pv.description, pv.license, pv.sha256, pv.size_bytes, pv.yanked, pv.created_at
        FROM package_versions pv
        JOIN packages p ON pv.package_id = p.id
        WHERE p.scope = $1 AND p.name = $2 AND pv.version = $3 AND pv.status = 'published'
        "#,
    )
    .bind(&scope)
    .bind(&name)
    .bind(&version)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?
    .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "NOT_FOUND", "version not found"))?;

    Ok(Json(PackageVersionDetail {
        scope,
        name,
        version: r.get("version"),
        description: r.get("description"),
        license: r.get("license"),
        sha256: r.get("sha256"),
        size_bytes: r.get("size_bytes"),
        yanked: r.get("yanked"),
        downloads: 0,
        dependencies: vec![],
        created_at: r.get("created_at"),
    }))
}

async fn download_version(
    State(state): State<Arc<AppState>>,
    Path((scope, name, version)): Path<(String, String, String)>,
) -> Result<Bytes, ApiError> {
    let r = sqlx::query(
        r#"
        SELECT pv.id, pv.storage_key
        FROM package_versions pv
        JOIN packages p ON pv.package_id = p.id
        WHERE p.scope = $1 AND p.name = $2 AND pv.version = $3 AND pv.status = 'published'
        "#,
    )
    .bind(&scope)
    .bind(&name)
    .bind(&version)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?
    .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "NOT_FOUND", "archive not found"))?;

    let pv_id: Uuid = r.get("id");
    let key: String = r.get("storage_key");
    let data = tokio::fs::read(state.storage_dir.join(&key))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "STORAGE_ERROR", e.to_string()))?;

    let _ = sqlx::query("INSERT INTO package_download_events (package_version_id) VALUES ($1)")
        .bind(pv_id)
        .execute(&state.db)
        .await;

    Ok(Bytes::from(data))
}

async fn publish_package(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<PublishResponse>, ApiError> {
    let user_id = authenticate(&state, &headers, SCOPE_PUBLISH).await?;

    let tmp = tempfile::tempdir()
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "IO_ERROR", e.to_string()))?;
    let sha256 = extract_archive(&body, tmp.path(), None)
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "INVALID_ARCHIVE", e.to_string()))?;

    let manifest = PackageManifest::from_file(&tmp.path().join("Block.toml"))
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "INVALID_MANIFEST", e.to_string()))?;
    let pkg_id = manifest
        .package_id()
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "INVALID_PACKAGE_ID", e.to_string()))?;

    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let ns_row = sqlx::query("SELECT id, owner_user_id FROM namespaces WHERE scope = $1")
        .bind(&pkg_id.scope)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    let ns_id: Uuid = match ns_row {
        Some(r) => {
            let owner: Option<Uuid> = r.get("owner_user_id");
            if owner != Some(user_id) {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "NAMESPACE_FORBIDDEN",
                    "you do not own this namespace",
                ));
            }
            r.get("id")
        }
        None => {
            let r = sqlx::query(
                "INSERT INTO namespaces (scope, owner_user_id) VALUES ($1, $2) RETURNING id",
            )
            .bind(&pkg_id.scope)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
            r.get("id")
        }
    };

    let pkg_row = sqlx::query(
        r#"
        INSERT INTO packages (namespace_id, scope, name, description, license)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (scope, name) DO UPDATE SET updated_at = NOW()
        RETURNING id
        "#,
    )
    .bind(ns_id)
    .bind(&pkg_id.scope)
    .bind(&pkg_id.name)
    .bind(&manifest.package.description)
    .bind(&manifest.package.license)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
    let package_id: Uuid = pkg_row.get("id");

    let storage_key = format!("{}/{}/{}.blockpkg", pkg_id.scope, pkg_id.name, manifest.package.version);
    let dest = state.storage_dir.join(&storage_key);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "STORAGE_ERROR", e.to_string()))?;
    }
    tokio::fs::write(&dest, &body)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "STORAGE_ERROR", e.to_string()))?;

    let pv_row = sqlx::query(
        r#"
        INSERT INTO package_versions (package_id, version, description, license, sha256, size_bytes, storage_key, published_by, status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'published')
        RETURNING id
        "#,
    )
    .bind(package_id)
    .bind(&manifest.package.version)
    .bind(&manifest.package.description)
    .bind(&manifest.package.license)
    .bind(&sha256)
    .bind(body.len() as i64)
    .bind(&storage_key)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| ApiError::new(StatusCode::CONFLICT, "VERSION_EXISTS", "package version already exists"))?;
    let pv_id: Uuid = pv_row.get("id");

    for (dep_name, req) in &manifest.dependencies {
        if let Ok(dep_id) = PackageId::parse(dep_name) {
            sqlx::query(
                "INSERT INTO package_dependencies (package_version_id, dep_scope, dep_name, version_req) VALUES ($1, $2, $3, $4)",
            )
            .bind(pv_id)
            .bind(&dep_id.scope)
            .bind(&dep_id.name)
            .bind(req)
            .execute(&mut *tx)
            .await
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;
        }
    }

    sqlx::query(
        "INSERT INTO publication_audit_records (package_id, version, actor_user_id, action, sha256) VALUES ($1, $2, $3, 'publish', $4)",
    )
    .bind(package_id)
    .bind(&manifest.package.version)
    .bind(user_id)
    .bind(&sha256)
    .execute(&mut *tx)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    Ok(Json(PublishResponse {
        scope: pkg_id.scope,
        name: pkg_id.name,
        version: manifest.package.version,
        sha256,
        status: "published".to_string(),
    }))
}

async fn yank_version(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((scope, name, version)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let user_id = authenticate(&state, &headers, SCOPE_YANK).await?;
    let res = sqlx::query(
        r#"
        UPDATE package_versions pv
        SET yanked = TRUE
        FROM packages p, namespaces n
        WHERE pv.package_id = p.id AND p.namespace_id = n.id
          AND p.scope = $1 AND p.name = $2 AND pv.version = $3 AND n.owner_user_id = $4
        "#,
    )
    .bind(&scope)
    .bind(&name)
    .bind(&version)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    if res.rows_affected() == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "NOT_FOUND", "version not found or forbidden"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn sparse_index(
    State(state): State<Arc<AppState>>,
    Path((scope, name)): Path<(String, String)>,
) -> Result<String, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT pv.id, pv.version, pv.sha256, pv.yanked
        FROM package_versions pv
        JOIN packages p ON pv.package_id = p.id
        WHERE p.scope = $1 AND p.name = $2 AND pv.status = 'published'
        ORDER BY pv.created_at ASC
        "#,
    )
    .bind(&scope)
    .bind(&name)
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

    if rows.is_empty() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "NOT_FOUND", "package not found"));
    }

    let mut lines = Vec::new();
    for r in rows {
        let pv_id: Uuid = r.get("id");
        let dep_rows = sqlx::query(
            "SELECT dep_scope, dep_name, version_req FROM package_dependencies WHERE package_version_id = $1",
        )
        .bind(pv_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()))?;

        let mut deps = BTreeMap::new();
        for d in dep_rows {
            let ds: String = d.get("dep_scope");
            let dn: String = d.get("dep_name");
            let req: String = d.get("version_req");
            deps.insert(format!("@{ds}/{dn}"), req);
        }

        let entry = IndexVersionEntry {
            name: format!("@{scope}/{name}"),
            vers: r.get("version"),
            deps,
            cksum: r.get("sha256"),
            yanked: r.get("yanked"),
        };
        lines.push(serde_json::to_string(&entry).unwrap_or_default());
    }
    Ok(lines.join("\n"))
}
