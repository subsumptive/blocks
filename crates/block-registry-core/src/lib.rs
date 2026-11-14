use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const SCOPE_READ: &str = "packages:read";
pub const SCOPE_PUBLISH: &str = "packages:publish";
pub const SCOPE_YANK: &str = "packages:yank";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageSummary {
    pub scope: String,
    pub name: String,
    pub latest_version: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub downloads: i64,
    pub deprecated: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageVersionDetail {
    pub scope: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub license: Option<String>,
    pub sha256: String,
    pub size_bytes: i64,
    pub yanked: bool,
    pub downloads: i64,
    pub dependencies: Vec<PackageDependencyDto>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDependencyDto {
    pub name: String,
    pub version_req: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedPackages {
    pub items: Vec<PackageSummary>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_in_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedTokenResponse {
    pub id: Uuid,
    pub name: String,
    pub token: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishResponse {
    pub scope: String,
    pub name: String,
    pub version: String,
    pub sha256: String,
    pub status: String,
}

pub fn hash_token(raw_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_token.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn has_scope(scopes: &[String], required: &str) -> bool {
    scopes.iter().any(|s| s == required)
}
