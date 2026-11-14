mod registry_cmds;

use anyhow::{bail, Context, Result};
use block_compiler::compile_file_with_packages;
use block_ir::BlockIr;
use block_package::PackageId;
use block_parser::TypeName;
use block_resolver::Lockfile;
use block_runtime::{execute, ComponentRegistry, ExecutionContext};
use block_types::Value;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "sub",
    version,
    about = "Subsumptive Blocks — Intelligence, unbundled."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new Block project
    New {
        /// Project directory name
        name: String,
    },
    /// Parse, validate, and type-check the project
    Check,
    /// Compile the project to a versioned IR artifact
    Build,
    /// Execute the project locally
    Run {
        /// Typed input values as key=value
        #[arg(long = "input", value_name = "KEY=VALUE")]
        inputs: Vec<String>,
        /// JSON file containing input values
        #[arg(long)]
        input_file: Option<PathBuf>,
    },
    /// Display block metadata, dependencies, inputs, outputs, and execution order
    Inspect,
    /// Run a deterministic local benchmark
    Bench {
        /// Number of iterations
        #[arg(long, default_value_t = 100)]
        iterations: usize,
        /// Typed input values as key=value
        #[arg(long = "input", value_name = "KEY=VALUE")]
        inputs: Vec<String>,
        /// JSON file containing input values
        #[arg(long)]
        input_file: Option<PathBuf>,
    },
    /// Authenticate with Subsumptive Registry
    Login {
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        registry: Option<String>,
    },
    /// Remove stored registry credentials
    Logout,
    /// Show the currently authenticated registry user
    Whoami,
    /// Search packages in Subsumptive Registry
    Search { query: String },
    /// Show package details
    Info { package: String },
    /// Add a dependency to Block.toml and install it
    Add { package: String },
    /// Remove a dependency from Block.toml
    Remove { package: String },
    /// Install dependencies from Block.toml / Block.lock
    Install {
        #[arg(long)]
        locked: bool,
        #[arg(long)]
        frozen: bool,
        #[arg(long)]
        offline: bool,
    },
    /// Update dependencies within manifest constraints
    Update { package: Option<String> },
    /// List resolved project dependencies
    List {
        #[arg(long)]
        tree: bool,
    },
    /// Validate and build a .blockpkg archive
    Package,
    /// Verify a .blockpkg archive's structure and integrity
    Verify { archive: PathBuf },
    /// Manage the local package cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Publish the current package to Subsumptive Registry
    Publish,
    /// Yank a published package version
    Yank { package_version: String },
}

#[derive(Debug, Deserialize, Serialize)]
struct BlockManifest {
    package: PackageSection,
    #[serde(default)]
    runtime: RuntimeSection,
}

#[derive(Subcommand)]
enum CacheAction {
    /// List cached packages
    List,
    /// Clean cached packages
    Clean {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct PackageSection {
    name: String,
    version: String,
    entry: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RuntimeSection {
    #[serde(default = "default_target")]
    target: String,
}

fn default_target() -> String {
    "local".to_string()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    if let Err(err) = run_cli() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::New { name } => cmd_new(&name),
        Commands::Check => cmd_check(),
        Commands::Build => {
            let _ = cmd_build()?;
            Ok(())
        }
        Commands::Run { inputs, input_file } => cmd_run(&inputs, input_file.as_deref()),
        Commands::Inspect => cmd_inspect(),
        Commands::Bench {
            iterations,
            inputs,
            input_file,
        } => cmd_bench(iterations, &inputs, input_file.as_deref()),
        Commands::Login { token, registry } => registry_cmds::cmd_login(token, registry),
        Commands::Logout => registry_cmds::cmd_logout(),
        Commands::Whoami => registry_cmds::cmd_whoami(),
        Commands::Search { query } => registry_cmds::cmd_search(&query),
        Commands::Info { package } => registry_cmds::cmd_info(&package),
        Commands::Add { package } => registry_cmds::cmd_add(&package),
        Commands::Remove { package } => registry_cmds::cmd_remove(&package),
        Commands::Install {
            locked,
            frozen,
            offline,
        } => registry_cmds::cmd_install(locked, frozen, offline),
        Commands::Update { package } => registry_cmds::cmd_update(package),
        Commands::List { tree } => registry_cmds::cmd_list(tree),
        Commands::Package => registry_cmds::cmd_package().map(|_| ()),
        Commands::Verify { archive } => registry_cmds::cmd_verify(&archive),
        Commands::Cache { action } => match action {
            CacheAction::List => registry_cmds::cmd_cache_list(),
            CacheAction::Clean { dry_run, yes } => registry_cmds::cmd_cache_clean(dry_run, yes),
        },
        Commands::Publish => registry_cmds::cmd_publish(),
        Commands::Yank { package_version } => registry_cmds::cmd_yank(&package_version),
    }
}

fn cmd_new(name: &str) -> Result<()> {
    let root = Path::new(name);
    if root.exists() {
        bail!("directory '{}' already exists", root.display());
    }
    fs::create_dir_all(root)?;

    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nentry = \"main.block\"\n\n[runtime]\ntarget = \"local\"\n"
    );
    fs::write(root.join("Block.toml"), manifest)?;

    let main_block = r#"block Greeting {
    input name: Text
    output message: Text

    use formatter = Format("Hello, {}")

    pipeline {
        message = formatter(name)
    }
}
"#;
    fs::write(root.join("main.block"), main_block)?;

    let readme = format!(
        "# {name}\n\nA Subsumptive Blocks project.\n\n## Usage\n\n```bash\nsub check\nsub build\nsub run --input name=World\n```\n"
    );
    fs::write(root.join("README.md"), readme)?;

    println!("Created Block project '{name}'");
    Ok(())
}

fn load_manifest() -> Result<BlockManifest> {
    let raw = fs::read_to_string("Block.toml")
        .context("failed to read Block.toml in current directory")?;
    let manifest: BlockManifest = toml::from_str(&raw).context("invalid Block.toml")?;
    if manifest.package.entry.trim().is_empty() {
        bail!("Block.toml: package.entry must not be empty");
    }
    Ok(manifest)
}

fn compile_project() -> Result<(BlockManifest, BlockIr)> {
    let manifest = load_manifest()?;
    let entry = &manifest.package.entry;
    let cwd = std::env::current_dir()?;
    let mut pkg_map = BTreeMap::new();
    let lock_path = cwd.join("Block.lock");
    if lock_path.exists() {
        let lf = Lockfile::read(&lock_path)?;
        let cache_root = cwd.join(".subsumptive").join("packages");
        for pkg in lf.package {
            if let Ok(id) = PackageId::parse(&pkg.name) {
                let dest = cache_root.join(&id.scope).join(&id.name).join(&pkg.version);
                pkg_map.insert(pkg.name, dest);
            }
        }
    }
    match compile_file_with_packages(Path::new(entry), &cwd, &pkg_map) {
        Ok(ir) => Ok((manifest, ir)),
        Err(err) => {
            let source = fs::read_to_string(entry).unwrap_or_default();
            eprintln!("{}", err.format_diagnostics(entry, &source));
            bail!("compilation failed");
        }
    }
}

fn cmd_check() -> Result<()> {
    let (manifest, ir) = compile_project()?;
    println!(
        "Checked {} ({}) — {} node(s), {} component(s)",
        manifest.package.name,
        ir.block_name,
        ir.nodes.len(),
        ir.components.len()
    );
    Ok(())
}

fn cmd_build() -> Result<BlockIr> {
    let (_manifest, ir) = compile_project()?;
    let out_dir = Path::new("target/block");
    fs::create_dir_all(out_dir)?;
    let out_path = out_dir.join("main.json");
    let json = ir.to_deterministic_json()?;
    fs::write(&out_path, json)?;
    println!("Built {}", out_path.display());
    Ok(ir)
}

fn parse_inputs(
    ir: &BlockIr,
    kv_inputs: &[String],
    input_file: Option<&Path>,
) -> Result<BTreeMap<String, Value>> {
    let mut raw_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    if let Some(path) = input_file {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read input file '{}'", path.display()))?;
        let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in '{}'", path.display()))?;
        raw_map.extend(parsed);
    }
    for item in kv_inputs {
        let (k, v) = item
            .split_once('=')
            .with_context(|| format!("invalid --input '{item}', expected KEY=VALUE"))?;
        raw_map.insert(k.to_string(), serde_json::Value::String(v.to_string()));
    }

    let mut typed = BTreeMap::new();
    for port in &ir.inputs {
        let jval = raw_map
            .get(&port.name)
            .with_context(|| format!("missing required input '{}'", port.name))?;
        let val = match port.ty {
            TypeName::Text => Value::Text(
                jval.as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| jval.to_string()),
            ),
            TypeName::Integer => {
                let n = if let Some(n) = jval.as_i64() {
                    n
                } else if let Some(s) = jval.as_str() {
                    s.parse::<i64>()?
                } else {
                    bail!("input '{}' expected Integer", port.name);
                };
                Value::Integer(n)
            }
            TypeName::Float => {
                let f = if let Some(f) = jval.as_f64() {
                    f
                } else if let Some(s) = jval.as_str() {
                    s.parse::<f64>()?
                } else {
                    bail!("input '{}' expected Float", port.name);
                };
                Value::Float(f)
            }
            TypeName::Boolean => {
                let b = if let Some(b) = jval.as_bool() {
                    b
                } else if let Some(s) = jval.as_str() {
                    s.parse::<bool>()?
                } else {
                    bail!("input '{}' expected Boolean", port.name);
                };
                Value::Boolean(b)
            }
            TypeName::Bytes => {
                let s = jval
                    .as_str()
                    .with_context(|| format!("input '{}' expected Bytes string", port.name))?;
                Value::Bytes(s.as_bytes().to_vec())
            }
            TypeName::TokenSequence => {
                let arr = jval.as_array().with_context(|| {
                    format!("input '{}' expected TokenSequence array", port.name)
                })?;
                let tokens = arr
                    .iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect();
                Value::TokenSequence(tokens)
            }
        };
        typed.insert(port.name.clone(), val);
    }
    Ok(typed)
}

fn cmd_run(kv_inputs: &[String], input_file: Option<&Path>) -> Result<()> {
    let ir = cmd_build()?;
    let inputs = parse_inputs(&ir, kv_inputs, input_file)?;
    let registry = ComponentRegistry::default();
    let ctx = ExecutionContext {
        project_root: std::env::current_dir()?,
        ..ExecutionContext::default()
    };
    let report = execute(&ir, &inputs, &registry, &ctx)?;
    for (k, v) in &report.outputs {
        match v {
            Value::Text(s) => println!("{k}: {s}"),
            Value::Integer(n) => println!("{k}: {n}"),
            Value::Float(f) => println!("{k}: {f}"),
            Value::Boolean(b) => println!("{k}: {b}"),
            Value::Bytes(b) => println!("{k}: <{} bytes>", b.len()),
            Value::TokenSequence(t) => println!("{k}: {:?}", t),
        }
    }
    Ok(())
}

fn cmd_inspect() -> Result<()> {
    let (manifest, ir) = compile_project()?;
    println!(
        "Package: {} v{}",
        manifest.package.name, manifest.package.version
    );
    println!("Block: {} (IR v{})", ir.block_name, ir.schema_version);
    println!("Inputs:");
    for inp in &ir.inputs {
        println!("  - {}: {}", inp.name, inp.ty);
    }
    println!("Outputs:");
    for out in &ir.outputs {
        println!("  - {}: {}", out.name, out.ty);
    }
    if !ir.modules.is_empty() {
        println!("Imported Modules:");
        for (alias, mod_ir) in &ir.modules {
            println!(
                "  - {alias} (block {}, {})",
                mod_ir.block_name, mod_ir.metadata.source_file
            );
        }
    }
    println!("Components:");
    for (alias, comp) in &ir.components {
        println!("  - {alias}: {} v{}", comp.component_id, comp.version);
    }
    println!("Execution order:");
    for node in &ir.nodes {
        println!(
            "  [{}] {} = {}({}) -> {}",
            node.id,
            node.target,
            node.instance_alias,
            node.args.join(", "),
            node.output_ty
        );
    }
    Ok(())
}

fn cmd_bench(iterations: usize, kv_inputs: &[String], input_file: Option<&Path>) -> Result<()> {
    if iterations == 0 {
        bail!("iterations must be >= 1");
    }
    let compile_start = std::time::Instant::now();
    let (_manifest, ir) = compile_project()?;
    let compile_time = compile_start.elapsed();

    let inputs = parse_inputs(&ir, kv_inputs, input_file)?;
    let registry = ComponentRegistry::default();
    let ctx = ExecutionContext {
        project_root: std::env::current_dir()?,
        ..ExecutionContext::default()
    };

    let mut durations: Vec<Duration> = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let report = execute(&ir, &inputs, &registry, &ctx)?;
        durations.push(report.total_duration);
    }
    durations.sort();

    let sum: Duration = durations.iter().sum();
    let mean = sum / (iterations as u32);
    let median = durations[iterations / 2];
    let p95_idx = ((iterations as f64) * 0.95).ceil() as usize;
    let p95 = durations[p95_idx.min(iterations - 1)];
    let min = durations[0];
    let max = durations[iterations - 1];

    println!("Benchmark ({} iterations)", iterations);
    println!("  Compile overhead: {:?}", compile_time);
    println!("  Mean:   {:?}", mean);
    println!("  Median: {:?}", median);
    println!("  p95:    {:?}", p95);
    println!("  Min:    {:?}", min);
    println!("  Max:    {:?}", max);
    Ok(())
}
