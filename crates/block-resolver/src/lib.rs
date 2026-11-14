use block_package::PackageId;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;
use thiserror::Error;

pub const LOCKFILE_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("package '{0}' not found in index")]
    PackageNotFound(String),
    #[error("no version of '{pkg}' satisfies requirement '{req}'")]
    NoMatchingVersion { pkg: String, req: String },
    #[error("version conflict for '{pkg}': '{req1}' vs '{req2}'")]
    VersionConflict {
        pkg: String,
        req1: String,
        req2: String,
    },
    #[error("circular dependency detected involving '{0}'")]
    CircularDependency(String),
    #[error("invalid semver: {0}")]
    Semver(#[from] semver::Error),
    #[error("lockfile error: {0}")]
    Lockfile(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexVersionEntry {
    pub name: String,
    pub vers: String,
    pub deps: BTreeMap<String, String>,
    pub cksum: String,
    pub yanked: bool,
}

pub trait PackageIndex {
    fn get_versions(&self, id: &PackageId) -> Result<Vec<IndexVersionEntry>, ResolveError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    pub version: u32,
    pub registry: String,
    #[serde(default)]
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub checksum: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}

impl Lockfile {
    pub fn read(path: &Path) -> Result<Self, ResolveError> {
        let content = fs::read_to_string(path)?;
        toml::from_str(&content).map_err(|e| ResolveError::Lockfile(e.to_string()))
    }

    pub fn write_atomic(&self, path: &Path) -> Result<(), ResolveError> {
        let content =
            toml::to_string_pretty(self).map_err(|e| ResolveError::Lockfile(e.to_string()))?;
        let tmp = path.with_extension("lock.tmp");
        fs::write(&tmp, content)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

pub fn resolve(
    root_deps: &BTreeMap<String, String>,
    registry_url: &str,
    index: &dyn PackageIndex,
) -> Result<Lockfile, ResolveError> {
    let mut selected: BTreeMap<String, (Version, IndexVersionEntry, String)> = BTreeMap::new();
    let mut visiting: HashSet<String> = HashSet::new();

    for (pkg_name, req_str) in root_deps {
        resolve_recursive(pkg_name, req_str, index, &mut selected, &mut visiting)?;
    }

    let packages = selected
        .into_iter()
        .map(|(name, (_ver, entry, _req))| LockedPackage {
            name,
            version: entry.vers,
            checksum: entry.cksum,
            dependencies: entry.deps,
        })
        .collect();

    Ok(Lockfile {
        version: LOCKFILE_VERSION,
        registry: registry_url.to_string(),
        package: packages,
    })
}

fn resolve_recursive(
    pkg_name: &str,
    req_str: &str,
    index: &dyn PackageIndex,
    selected: &mut BTreeMap<String, (Version, IndexVersionEntry, String)>,
    visiting: &mut HashSet<String>,
) -> Result<(), ResolveError> {
    if visiting.contains(pkg_name) {
        return Err(ResolveError::CircularDependency(pkg_name.to_string()));
    }
    let req = VersionReq::parse(req_str)?;

    if let Some((existing_ver, _entry, existing_req)) = selected.get(pkg_name) {
        if req.matches(existing_ver) {
            return Ok(());
        } else {
            return Err(ResolveError::VersionConflict {
                pkg: pkg_name.to_string(),
                req1: existing_req.clone(),
                req2: req_str.to_string(),
            });
        }
    }

    let id = PackageId::parse(pkg_name)
        .map_err(|_| ResolveError::PackageNotFound(pkg_name.to_string()))?;
    let mut entries = index.get_versions(&id)?;

    // Sort descending by semver, excluding yanked versions
    let mut candidates: Vec<(Version, IndexVersionEntry)> = entries
        .drain(..)
        .filter(|e| !e.yanked)
        .filter_map(|e| Version::parse(&e.vers).ok().map(|v| (v, e)))
        .filter(|(v, _)| req.matches(v))
        .collect();

    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    let (chosen_ver, chosen_entry) =
        candidates
            .into_iter()
            .next()
            .ok_or_else(|| ResolveError::NoMatchingVersion {
                pkg: pkg_name.to_string(),
                req: req_str.to_string(),
            })?;

    visiting.insert(pkg_name.to_string());
    for (dep_name, dep_req) in &chosen_entry.deps {
        resolve_recursive(dep_name, dep_req, index, selected, visiting)?;
    }
    visiting.remove(pkg_name);

    selected.insert(
        pkg_name.to_string(),
        (chosen_ver, chosen_entry, req_str.to_string()),
    );
    Ok(())
}

pub struct InMemoryIndex {
    pub packages: HashMap<String, Vec<IndexVersionEntry>>,
}

impl PackageIndex for InMemoryIndex {
    fn get_versions(&self, id: &PackageId) -> Result<Vec<IndexVersionEntry>, ResolveError> {
        self.packages
            .get(&id.to_string())
            .cloned()
            .ok_or_else(|| ResolveError::PackageNotFound(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_transitive_and_skips_yanked() {
        let mut packages = HashMap::new();
        packages.insert(
            "@subsumptive/pkg-a".to_string(),
            vec![
                IndexVersionEntry {
                    name: "@subsumptive/pkg-a".to_string(),
                    vers: "0.1.0".to_string(),
                    deps: BTreeMap::from([(
                        "@subsumptive/pkg-b".to_string(),
                        "^0.2.0".to_string(),
                    )]),
                    cksum: "aaa".to_string(),
                    yanked: false,
                },
                IndexVersionEntry {
                    name: "@subsumptive/pkg-a".to_string(),
                    vers: "0.2.0".to_string(),
                    deps: BTreeMap::new(),
                    cksum: "bbb".to_string(),
                    yanked: true,
                },
            ],
        );
        packages.insert(
            "@subsumptive/pkg-b".to_string(),
            vec![IndexVersionEntry {
                name: "@subsumptive/pkg-b".to_string(),
                vers: "0.2.1".to_string(),
                deps: BTreeMap::new(),
                cksum: "ccc".to_string(),
                yanked: false,
            }],
        );
        let idx = InMemoryIndex { packages };
        let root = BTreeMap::from([("@subsumptive/pkg-a".to_string(), "^0.1.0".to_string())]);
        let lock = resolve(&root, "https://registry.subsumptive.net", &idx).unwrap();
        assert_eq!(lock.package.len(), 2);
        assert_eq!(lock.package[0].version, "0.1.0");
        assert_eq!(lock.package[1].version, "0.2.1");
    }

    #[test]
    fn detects_circular_dependency() {
        let mut packages = HashMap::new();
        packages.insert(
            "@subsumptive/pkg-a".to_string(),
            vec![IndexVersionEntry {
                name: "@subsumptive/pkg-a".to_string(),
                vers: "0.1.0".to_string(),
                deps: BTreeMap::from([("@subsumptive/pkg-b".to_string(), "0.1.0".to_string())]),
                cksum: "aaa".to_string(),
                yanked: false,
            }],
        );
        packages.insert(
            "@subsumptive/pkg-b".to_string(),
            vec![IndexVersionEntry {
                name: "@subsumptive/pkg-b".to_string(),
                vers: "0.1.0".to_string(),
                deps: BTreeMap::from([("@subsumptive/pkg-a".to_string(), "0.1.0".to_string())]),
                cksum: "bbb".to_string(),
                yanked: false,
            }],
        );
        let idx = InMemoryIndex { packages };
        let root = BTreeMap::from([("@subsumptive/pkg-a".to_string(), "0.1.0".to_string())]);
        assert!(matches!(
            resolve(&root, "https://registry.subsumptive.net", &idx),
            Err(ResolveError::CircularDependency(_))
        ));
    }
}
