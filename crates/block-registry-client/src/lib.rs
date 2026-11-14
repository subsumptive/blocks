use block_package::PackageId;
use block_registry_core::{
    ApiErrorBody, PackageSummary, PackageVersionDetail, PaginatedPackages, PublishResponse,
    UserProfile,
};
use block_resolver::{IndexVersionEntry, PackageIndex, ResolveError};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

pub const DEFAULT_REGISTRY_URL: &str = "https://registry.subsumptive.net";

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("registry error [{code}]: {message}")]
    Api { code: String, message: String },
    #[error("not authenticated; run `sub login` first")]
    NotAuthenticated,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub registry: String,
    pub token: String,
}

pub fn credentials_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| {
            PathBuf::from(h)
                .join(".subsumptive")
                .join("credentials.json")
        })
}

pub fn save_credentials(creds: &Credentials) -> Result<PathBuf, ClientError> {
    let path = credentials_path().ok_or_else(|| {
        ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "home directory not found",
        ))
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(creds)?;
    fs::write(&path, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

pub fn load_credentials() -> Result<Option<Credentials>, ClientError> {
    let Some(path) = credentials_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(path)?;
    let creds: Credentials = serde_json::from_str(&data)?;
    Ok(Some(creds))
}

pub fn remove_credentials() -> Result<(), ClientError> {
    if let Some(path) = credentials_path() {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub struct RegistryClient {
    base_url: String,
    token: Option<String>,
    http: Client,
}

impl RegistryClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
            http: Client::builder()
                .user_agent("sub-cli/0.1.0")
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    pub fn from_env_or_creds() -> Self {
        let env_url = std::env::var("SUBSUMPTIVE_REGISTRY_URL").ok();
        let env_token = std::env::var("SUBSUMPTIVE_TOKEN").ok();
        let creds = load_credentials().ok().flatten();

        let url = env_url
            .or_else(|| creds.as_ref().map(|c| c.registry.clone()))
            .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());
        let token = env_token.or_else(|| creds.map(|c| c.token));
        Self::new(url, token)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn whoami(&self) -> Result<UserProfile, ClientError> {
        let token = self.token.as_deref().ok_or(ClientError::NotAuthenticated)?;
        let resp = self
            .http
            .get(format!("{}/api/v1/me", self.base_url))
            .bearer_auth(token)
            .send()?;
        parse_json_or_err(resp)
    }

    pub fn search(
        &self,
        query: &str,
        page: i64,
        per_page: i64,
    ) -> Result<PaginatedPackages, ClientError> {
        let resp = self
            .http
            .get(format!("{}/api/v1/packages/search", self.base_url))
            .query(&[
                ("q", query),
                ("page", &page.to_string()),
                ("per_page", &per_page.to_string()),
            ])
            .send()?;
        parse_json_or_err(resp)
    }

    pub fn get_package(&self, id: &PackageId) -> Result<PackageSummary, ClientError> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/packages/{}/{}",
                self.base_url, id.scope, id.name
            ))
            .send()?;
        parse_json_or_err(resp)
    }

    pub fn get_versions(&self, id: &PackageId) -> Result<Vec<PackageVersionDetail>, ClientError> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/packages/{}/{}/versions",
                self.base_url, id.scope, id.name
            ))
            .send()?;
        parse_json_or_err(resp)
    }

    pub fn download_archive(&self, id: &PackageId, version: &str) -> Result<Vec<u8>, ClientError> {
        let resp = self
            .http
            .get(format!(
                "{}/api/v1/packages/{}/{}/{}/download",
                self.base_url, id.scope, id.name, version
            ))
            .send()?;
        if !resp.status().is_success() {
            let err: ApiErrorBody = resp.json()?;
            return Err(ClientError::Api {
                code: err.error.code,
                message: err.error.message,
            });
        }
        Ok(resp.bytes()?.to_vec())
    }

    pub fn publish(&self, archive_bytes: Vec<u8>) -> Result<PublishResponse, ClientError> {
        let token = self.token.as_deref().ok_or(ClientError::NotAuthenticated)?;
        let resp = self
            .http
            .post(format!("{}/api/v1/packages/publish", self.base_url))
            .bearer_auth(token)
            .header("content-type", "application/octet-stream")
            .body(archive_bytes)
            .send()?;
        parse_json_or_err(resp)
    }

    pub fn yank(&self, id: &PackageId, version: &str) -> Result<(), ClientError> {
        let token = self.token.as_deref().ok_or(ClientError::NotAuthenticated)?;
        let resp = self
            .http
            .post(format!(
                "{}/api/v1/packages/{}/{}/{}/yank",
                self.base_url, id.scope, id.name, version
            ))
            .bearer_auth(token)
            .send()?;
        if !resp.status().is_success() {
            let err: ApiErrorBody = resp.json()?;
            return Err(ClientError::Api {
                code: err.error.code,
                message: err.error.message,
            });
        }
        Ok(())
    }
}

impl PackageIndex for RegistryClient {
    fn get_versions(&self, id: &PackageId) -> Result<Vec<IndexVersionEntry>, ResolveError> {
        let url = format!("{}/index/{}/{}", self.base_url, id.scope, id.name);
        let resp = self
            .http
            .get(&url)
            .send()
            .map_err(|e| ResolveError::Lockfile(e.to_string()))?;
        if resp.status().as_u16() == 404 {
            return Err(ResolveError::PackageNotFound(id.to_string()));
        }
        let text = resp
            .text()
            .map_err(|e| ResolveError::Lockfile(e.to_string()))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let entry: IndexVersionEntry =
                serde_json::from_str(trimmed).map_err(|e| ResolveError::Lockfile(e.to_string()))?;
            entries.push(entry);
        }
        Ok(entries)
    }
}

fn parse_json_or_err<T: serde::de::DeserializeOwned>(
    resp: reqwest::blocking::Response,
) -> Result<T, ClientError> {
    if !resp.status().is_success() {
        let err: ApiErrorBody = resp.json()?;
        return Err(ClientError::Api {
            code: err.error.code,
            message: err.error.message,
        });
    }
    Ok(resp.json()?)
}
