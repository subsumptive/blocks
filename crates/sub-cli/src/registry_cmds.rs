use anyhow::{bail, Context, Result};
use block_package::{build_archive, extract_archive, PackageId, PackageManifest};
use block_registry_client::{
    remove_credentials, save_credentials, Credentials, RegistryClient, DEFAULT_REGISTRY_URL,
};
use block_resolver::{resolve, Lockfile};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub fn cmd_login(token: Option<String>, registry: Option<String>) -> Result<()> {
    let reg_url = registry
        .or_else(|| std::env::var("SUBSUMPTIVE_REGISTRY_URL").ok())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());

    let tok = match token {
        Some(t) => t,
        None => {
            println!("Open {reg_url}/settings/tokens to create a scoped API token,");
            println!("or pass `--token <TOKEN>` directly.");
            bail!("interactive token prompt requires --token <TOKEN> in non-TTY mode");
        }
    };

    let client = RegistryClient::new(&reg_url, Some(tok.clone()));
    let me = client
        .whoami()
        .context("failed to verify token with registry")?;
    let path = save_credentials(&Credentials {
        registry: reg_url,
        token: tok,
    })?;
    println!(
        "Logged in as @{} (credentials saved to {})",
        me.username,
        path.display()
    );
    Ok(())
}

pub fn cmd_logout() -> Result<()> {
    remove_credentials()?;
    println!("Logged out");
    Ok(())
}

pub fn cmd_whoami() -> Result<()> {
    let client = RegistryClient::from_env_or_creds();
    let me = client.whoami()?;
    println!("@{} ({})", me.username, client.base_url());
    Ok(())
}

pub fn cmd_search(query: &str) -> Result<()> {
    let client = RegistryClient::from_env_or_creds();
    let res = client.search(query, 1, 20)?;
    if res.items.is_empty() {
        println!("No packages found matching '{query}'.");
        return Ok(());
    }
    for pkg in res.items {
        let ver = pkg.latest_version.unwrap_or_else(|| "-".to_string());
        let desc = pkg.description.unwrap_or_default();
        println!("@{}/{} ({}) - {}", pkg.scope, pkg.name, ver, desc);
    }
    Ok(())
}

pub fn cmd_info(package: &str) -> Result<()> {
    let id = PackageId::parse(package)?;
    let client = RegistryClient::from_env_or_creds();
    let pkg = client.get_package(&id)?;
    println!("Package: @{}/{}", pkg.scope, pkg.name);
    println!(
        "Latest:  {}",
        pkg.latest_version.unwrap_or_else(|| "-".into())
    );
    if let Some(desc) = pkg.description {
        println!("Desc:    {desc}");
    }
    if let Some(lic) = pkg.license {
        println!("License: {lic}");
    }
    println!("Downloads: {}", pkg.downloads);
    Ok(())
}

pub fn cmd_package() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let manifest = PackageManifest::from_file(&cwd.join("Block.toml"))?;
    let id = manifest.package_id()?;
    let out_name = format!(
        "{}-{}-{}.blockpkg",
        id.scope, id.name, manifest.package.version
    );
    let out_path = cwd.join("target/package").join(out_name);
    let res = build_archive(&cwd, &out_path)?;
    println!(
        "Packaged {} v{} -> {} ({} bytes, sha256: {})",
        id,
        manifest.package.version,
        res.archive_path.display(),
        res.size_bytes,
        res.sha256
    );
    Ok(res.archive_path)
}

pub fn cmd_publish() -> Result<()> {
    let archive_path = cmd_package()?;
    let bytes = fs::read(&archive_path)?;
    let client = RegistryClient::from_env_or_creds();
    let res = client.publish(bytes)?;
    println!(
        "Published @{}/{} v{} (sha256: {})",
        res.scope, res.name, res.version, res.sha256
    );
    Ok(())
}

pub fn cmd_yank(pkg_ver: &str) -> Result<()> {
    let (pkg_str, ver) = pkg_ver
        .rsplit_once('@')
        .context("expected format @scope/name@version")?;
    let id = PackageId::parse(pkg_str)?;
    let client = RegistryClient::from_env_or_creds();
    client.yank(&id, ver)?;
    println!("Yanked {id}@{ver}");
    Ok(())
}

pub fn cmd_add(pkg_spec: &str) -> Result<()> {
    let (pkg_str, req_str) = if let Some((p, v)) = pkg_spec.rsplit_once('@') {
        if p.is_empty() {
            (pkg_spec, "*")
        } else {
            (p, v)
        }
    } else {
        (pkg_spec, "*")
    };
    let id = PackageId::parse(pkg_str)?;
    let cwd = std::env::current_dir()?;
    let manifest_path = cwd.join("Block.toml");
    let mut manifest = PackageManifest::from_file(&manifest_path)?;
    manifest
        .dependencies
        .insert(id.to_string(), req_str.to_string());
    let updated = toml::to_string_pretty(&manifest)?;
    fs::write(&manifest_path, updated)?;
    println!("Added {id} ({req_str}) to Block.toml");
    cmd_install(false, false, false)
}

pub fn cmd_remove(pkg_str: &str) -> Result<()> {
    let id = PackageId::parse(pkg_str)?;
    let cwd = std::env::current_dir()?;
    let manifest_path = cwd.join("Block.toml");
    let mut manifest = PackageManifest::from_file(&manifest_path)?;
    if manifest.dependencies.remove(&id.to_string()).is_none() {
        bail!("dependency {id} not found in Block.toml");
    }
    let updated = toml::to_string_pretty(&manifest)?;
    fs::write(&manifest_path, updated)?;
    println!("Removed {id} from Block.toml");
    cmd_install(false, false, false)
}

pub fn cmd_install(locked: bool, frozen: bool, offline: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let manifest = PackageManifest::from_file(&cwd.join("Block.toml"))?;
    let lock_path = cwd.join("Block.lock");
    let client = RegistryClient::from_env_or_creds();

    let lockfile = if locked || frozen {
        if !lock_path.exists() {
            bail!("--locked/--frozen passed but Block.lock does not exist");
        }
        let lf = Lockfile::read(&lock_path)?;
        let locked_names: BTreeMap<String, String> = lf
            .package
            .iter()
            .map(|p| (p.name.clone(), p.version.clone()))
            .collect();
        for dep in manifest.dependencies.keys() {
            if !locked_names.contains_key(dep) {
                bail!("Block.lock is out of date: missing dependency '{dep}'");
            }
        }
        lf
    } else if offline {
        if !lock_path.exists() {
            bail!("--offline passed but Block.lock does not exist");
        }
        Lockfile::read(&lock_path)?
    } else {
        let lf = resolve(&manifest.dependencies, client.base_url(), &client)?;
        lf.write_atomic(&lock_path)?;
        lf
    };

    let cache_root = cwd.join(".subsumptive").join("packages");
    for pkg in &lockfile.package {
        let id = PackageId::parse(&pkg.name)?;
        let dest = cache_root.join(&id.scope).join(&id.name).join(&pkg.version);
        if dest.exists() {
            continue;
        }
        if frozen || offline {
            bail!(
                "package {} v{} is not in local cache and network access is disabled (--frozen/--offline)",
                id,
                pkg.version
            );
        }
        let bytes = client.download_archive(&id, &pkg.version)?;
        extract_archive(&bytes, &dest, Some(&pkg.checksum))?;
        println!("Installed {} v{}", id, pkg.version);
    }
    println!(
        "Dependencies up to date ({} package(s))",
        lockfile.package.len()
    );
    Ok(())
}

pub fn cmd_update(package: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let manifest = PackageManifest::from_file(&cwd.join("Block.toml"))?;
    let client = RegistryClient::from_env_or_creds();
    let lf = resolve(&manifest.dependencies, client.base_url(), &client)?;
    lf.write_atomic(&cwd.join("Block.lock"))?;
    if let Some(pkg) = package {
        println!("Updated dependency graph (requested: {pkg})");
    } else {
        println!("Updated Block.lock ({} package(s))", lf.package.len());
    }
    cmd_install(true, false, false)
}

pub fn cmd_list(tree: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let lock_path = cwd.join("Block.lock");
    if !lock_path.exists() {
        println!("No Block.lock found (0 dependencies).");
        return Ok(());
    }
    let lf = Lockfile::read(&lock_path)?;
    for pkg in &lf.package {
        println!("{} v{}", pkg.name, pkg.version);
        if tree {
            for (dep, req) in &pkg.dependencies {
                println!("  └── {dep} ({req})");
            }
        }
    }
    Ok(())
}

pub fn cmd_verify(archive: &std::path::Path) -> Result<()> {
    let res = block_package::verify_archive(archive)?;
    let full_name = match res.scope {
        Some(s) => format!("@{}/{}", s, res.name),
        None => res.name,
    };
    println!(
        "Verified {} v{} (entry: {}, {} bytes, sha256: {})",
        full_name, res.version, res.entry, res.size_bytes, res.sha256
    );
    Ok(())
}

pub fn cmd_cache_list() -> Result<()> {
    let cache_dir = std::env::current_dir()?
        .join(".subsumptive")
        .join("packages");
    if !cache_dir.exists() {
        println!("Local package cache is empty.");
        return Ok(());
    }
    println!("Cache directory: {}", cache_dir.display());
    for entry in fs::read_dir(&cache_dir)? {
        let entry = entry?;
        println!("  {}", entry.file_name().to_string_lossy());
    }
    Ok(())
}

pub fn cmd_cache_clean(dry_run: bool, yes: bool) -> Result<()> {
    let cache_dir = std::env::current_dir()?
        .join(".subsumptive")
        .join("packages");
    if !cache_dir.exists() {
        println!("Local package cache is already empty.");
        return Ok(());
    }
    if dry_run {
        println!("Dry run: would remove {}", cache_dir.display());
        return Ok(());
    }
    if !yes {
        bail!("pass --yes to confirm removing {}", cache_dir.display());
    }
    fs::remove_dir_all(&cache_dir)?;
    println!("Cleaned {}", cache_dir.display());
    Ok(())
}
