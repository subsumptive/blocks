use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Builder, EntryType, Header};
use thiserror::Error;
use walkdir::WalkDir;

pub const BLOCKPKG_FORMAT_VERSION: u32 = 1;
pub const MAX_ARCHIVE_BYTES: usize = 10 * 1024 * 1024; // 10 MiB
pub const MAX_ENTRY_BYTES: usize = 5 * 1024 * 1024; // 5 MiB
pub const MAX_ENTRIES: usize = 512;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("invalid package identifier '{0}': expected @scope/name with lowercase alphanumeric or hyphens")]
    InvalidPackageId(String),
    #[error(
        "invalid scope or name '{0}': must be 2..=64 lowercase alphanumeric or hyphen characters"
    )]
    InvalidIdentifierPart(String),
    #[error("invalid semver version '{0}': {1}")]
    InvalidVersion(String, semver::Error),
    #[error("invalid dependency requirement '{0}': {1}")]
    InvalidVersionReq(String, semver::Error),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("archive security violation: {0}")]
    SecurityViolation(String),
    #[error("archive size limit exceeded (max {MAX_ARCHIVE_BYTES} bytes)")]
    ArchiveTooLarge,
    #[error("integrity mismatch: expected {expected}, got {actual}")]
    IntegrityMismatch { expected: String, actual: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PackageId {
    pub scope: String,
    pub name: String,
}

impl PackageId {
    pub fn parse(raw: &str) -> Result<Self, PackageError> {
        let trimmed = raw.trim();
        let rest = trimmed
            .strip_prefix('@')
            .ok_or_else(|| PackageError::InvalidPackageId(raw.to_string()))?;
        let (scope, name) = rest
            .split_once('/')
            .ok_or_else(|| PackageError::InvalidPackageId(raw.to_string()))?;
        validate_ident_part(scope)?;
        validate_ident_part(name)?;
        Ok(Self {
            scope: scope.to_ascii_lowercase(),
            name: name.to_ascii_lowercase(),
        })
    }

    pub fn new(scope: &str, name: &str) -> Result<Self, PackageError> {
        validate_ident_part(scope)?;
        validate_ident_part(name)?;
        Ok(Self {
            scope: scope.to_ascii_lowercase(),
            name: name.to_ascii_lowercase(),
        })
    }
}

impl std::fmt::Display for PackageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}/{}", self.scope, self.name)
    }
}

pub fn validate_ident_part(part: &str) -> Result<(), PackageError> {
    let len = part.len();
    if !(2..=64).contains(&len) {
        return Err(PackageError::InvalidIdentifierPart(part.to_string()));
    }
    let bytes = part.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() || !bytes[len - 1].is_ascii_alphanumeric() {
        return Err(PackageError::InvalidIdentifierPart(part.to_string()));
    }
    if !part
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(PackageError::InvalidIdentifierPart(part.to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub package: PackageMetadata,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    pub name: String,
    #[serde(default)]
    pub scope: Option<String>,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub entry: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_target")]
    pub target: String,
}

fn default_target() -> String {
    "local".to_string()
}

impl PackageManifest {
    pub fn from_file(path: &Path) -> Result<Self, PackageError> {
        let content = fs::read_to_string(path)?;
        Self::parse(&content)
    }

    pub fn parse(content: &str) -> Result<Self, PackageError> {
        let manifest: Self =
            toml::from_str(content).map_err(|e| PackageError::Manifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), PackageError> {
        validate_ident_part(&self.package.name)?;
        if let Some(scope) = &self.package.scope {
            validate_ident_part(scope)?;
        }
        Version::parse(&self.package.version)
            .map_err(|e| PackageError::InvalidVersion(self.package.version.clone(), e))?;
        if self.package.entry.trim().is_empty() {
            return Err(PackageError::Manifest(
                "package.entry cannot be empty".into(),
            ));
        }
        for (dep, req) in &self.dependencies {
            PackageId::parse(dep)?;
            VersionReq::parse(req).map_err(|e| PackageError::InvalidVersionReq(req.clone(), e))?;
        }
        Ok(())
    }

    pub fn package_id(&self) -> Result<PackageId, PackageError> {
        let scope = self.package.scope.as_deref().ok_or_else(|| {
            PackageError::Manifest("package.scope is required for publishing".into())
        })?;
        PackageId::new(scope, &self.package.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveBuildResult {
    pub archive_path: PathBuf,
    pub sha256: String,
    pub size_bytes: usize,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveVerifyResult {
    pub name: String,
    pub scope: Option<String>,
    pub version: String,
    pub entry: String,
    pub sha256: String,
    pub size_bytes: usize,
}

pub fn verify_archive(path: &Path) -> Result<ArchiveVerifyResult, PackageError> {
    let bytes = fs::read(path)?;
    let tmp = tempfile::Builder::new().prefix("sub-verify-").tempdir()?;
    let sha256 = extract_archive(&bytes, tmp.path(), None)?;
    let manifest_path = tmp.path().join("Block.toml");
    if !manifest_path.exists() {
        return Err(PackageError::Manifest("archive missing Block.toml".into()));
    }
    let manifest = PackageManifest::from_file(&manifest_path)?;
    let entry_path = tmp.path().join(&manifest.package.entry);
    if !entry_path.exists() {
        return Err(PackageError::Manifest(format!(
            "archive missing declared entrypoint '{}'",
            manifest.package.entry
        )));
    }
    Ok(ArchiveVerifyResult {
        name: manifest.package.name,
        scope: manifest.package.scope,
        version: manifest.package.version,
        entry: manifest.package.entry,
        sha256,
        size_bytes: bytes.len(),
    })
}

fn is_excluded(rel: &Path) -> bool {
    for comp in rel.components() {
        if let Component::Normal(os) = comp {
            let s = os.to_string_lossy();
            if s == "target"
                || s == ".git"
                || s == ".env"
                || s.starts_with(".env.")
                || s == ".subsumptive"
                || s == "node_modules"
            {
                return true;
            }
        }
    }
    false
}

pub fn build_archive(
    project_dir: &Path,
    output_path: &Path,
) -> Result<ArchiveBuildResult, PackageError> {
    let manifest_path = project_dir.join("Block.toml");
    let manifest = PackageManifest::from_file(&manifest_path)?;
    manifest.validate()?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in WalkDir::new(project_dir).follow_links(false) {
        let entry = entry.map_err(|e| PackageError::Io(std::io::Error::other(e)))?;
        let path = entry.path();
        if path == project_dir {
            continue;
        }
        let rel = path
            .strip_prefix(project_dir)
            .map_err(|e| PackageError::SecurityViolation(e.to_string()))?;
        if is_excluded(rel) {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(PackageError::SecurityViolation(format!(
                "symlinks are not allowed in .blockpkg archives: {}",
                rel.display()
            )));
        }
        if entry.file_type().is_file() {
            let norm = normalize_rel_path(rel)?;
            let data = fs::read(path)?;
            if data.len() > MAX_ENTRY_BYTES {
                return Err(PackageError::SecurityViolation(format!(
                    "file '{}' exceeds max entry size of {MAX_ENTRY_BYTES} bytes",
                    norm
                )));
            }
            files.push((norm, data));
        }
    }

    // Deterministic ordering
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.len() > MAX_ENTRIES {
        return Err(PackageError::SecurityViolation(format!(
            "archive exceeds max entry count of {MAX_ENTRIES}"
        )));
    }

    let mut buf = Vec::new();
    {
        let encoder = GzEncoder::new(&mut buf, Compression::default());
        let mut builder = Builder::new(encoder);

        // Add format version metadata
        let meta = format!("{BLOCKPKG_FORMAT_VERSION}\n");
        append_bytes(&mut builder, ".blockpkg-version", meta.as_bytes())?;

        for (rel_path, data) in &files {
            append_bytes(&mut builder, rel_path, data)?;
        }
        builder.finish()?;
    }

    if buf.len() > MAX_ARCHIVE_BYTES {
        return Err(PackageError::ArchiveTooLarge);
    }

    let sha256 = sha256_hex(&buf);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, &buf)?;

    Ok(ArchiveBuildResult {
        archive_path: output_path.to_path_buf(),
        sha256,
        size_bytes: buf.len(),
        file_count: files.len(),
    })
}

fn append_bytes<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    data: &[u8],
) -> Result<(), PackageError> {
    let mut header = Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    builder.append_data(&mut header, path, data)?;
    Ok(())
}

pub fn extract_archive(
    archive_bytes: &[u8],
    dest_dir: &Path,
    expected_sha256: Option<&str>,
) -> Result<String, PackageError> {
    if archive_bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(PackageError::ArchiveTooLarge);
    }
    let actual_sha256 = sha256_hex(archive_bytes);
    if let Some(expected) = expected_sha256 {
        if !actual_sha256.eq_ignore_ascii_case(expected) {
            return Err(PackageError::IntegrityMismatch {
                expected: expected.to_string(),
                actual: actual_sha256,
            });
        }
    }

    let gz = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(gz);
    let mut seen_paths = HashSet::new();
    let mut entry_count = 0usize;

    fs::create_dir_all(dest_dir)?;

    for entry_res in archive.entries()? {
        entry_count += 1;
        if entry_count > MAX_ENTRIES {
            return Err(PackageError::SecurityViolation(format!(
                "archive exceeds max entry count of {MAX_ENTRIES}"
            )));
        }
        let mut entry = entry_res?;
        let entry_type = entry.header().entry_type();
        if entry_type == EntryType::Symlink || entry_type == EntryType::Link {
            return Err(PackageError::SecurityViolation(
                "symlinks and hardlinks are forbidden in .blockpkg".into(),
            ));
        }
        if entry_type != EntryType::Regular {
            continue;
        }

        let raw_path = entry.path()?.into_owned();
        let norm = normalize_rel_path(&raw_path)?;
        if !seen_paths.insert(norm.clone()) {
            return Err(PackageError::SecurityViolation(format!(
                "duplicate archive path '{norm}'"
            )));
        }

        let size = entry.header().size()? as usize;
        if size > MAX_ENTRY_BYTES {
            return Err(PackageError::SecurityViolation(format!(
                "archive entry '{norm}' exceeds max entry size"
            )));
        }

        let mut data = Vec::with_capacity(size);
        entry
            .by_ref()
            .take((MAX_ENTRY_BYTES + 1) as u64)
            .read_to_end(&mut data)?;
        if data.len() > MAX_ENTRY_BYTES {
            return Err(PackageError::SecurityViolation(format!(
                "archive entry '{norm}' exceeds max decompressed size"
            )));
        }

        let out_path = dest_dir.join(&norm);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&out_path, &data)?;
    }

    Ok(actual_sha256)
}

fn normalize_rel_path(path: &Path) -> Result<String, PackageError> {
    if path.is_absolute() {
        return Err(PackageError::SecurityViolation(format!(
            "absolute path forbidden: {}",
            path.display()
        )));
    }
    let mut parts = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Normal(s) => parts.push(s.to_string_lossy().to_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PackageError::SecurityViolation(format!(
                    "unsafe path component in '{}'",
                    path.display()
                )))
            }
        }
    }
    if parts.is_empty() {
        return Err(PackageError::SecurityViolation("empty archive path".into()));
    }
    Ok(parts.join("/"))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn validates_package_id() {
        let id = PackageId::parse("@subsumptive/tokenizer-basic").unwrap();
        assert_eq!(id.scope, "subsumptive");
        assert_eq!(id.name, "tokenizer-basic");
        assert!(PackageId::parse("subsumptive/tokenizer").is_err());
        assert!(PackageId::parse("@Subsumptive/Tokenizer").is_err());
    }

    #[test]
    fn builds_and_extracts_deterministic_archive() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("pkg");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("Block.toml"),
            r#"
[package]
name = "tokenizer-basic"
scope = "subsumptive"
version = "0.1.0"
entry = "main.block"
"#,
        )
        .unwrap();
        fs::write(
            proj.join("main.block"),
            "block T { input x: Text output y: Text pipeline { y = x } }\n",
        )
        .unwrap();

        let out1 = dir.path().join("a.blockpkg");
        let out2 = dir.path().join("b.blockpkg");
        let r1 = build_archive(&proj, &out1).unwrap();
        let r2 = build_archive(&proj, &out2).unwrap();
        assert_eq!(r1.sha256, r2.sha256);

        let extract_dir = dir.path().join("out");
        let bytes = fs::read(&out1).unwrap();
        let hash = extract_archive(&bytes, &extract_dir, Some(&r1.sha256)).unwrap();
        assert_eq!(hash, r1.sha256);
        assert!(extract_dir.join("Block.toml").exists());
    }

    #[test]
    fn rejects_path_traversal_archive() {
        assert!(normalize_rel_path(Path::new("../evil.txt")).is_err());
        assert!(normalize_rel_path(Path::new("/etc/passwd")).is_err());
        assert!(normalize_rel_path(Path::new("src/../../evil.txt")).is_err());
    }
}
