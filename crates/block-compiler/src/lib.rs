use block_ir::{BlockIr, IrComponentInstance, IrMetadata, IrNode, IrPort, IR_SCHEMA_VERSION};
use block_lexer::{tokenize, LexError};
use block_parser::{parse, ParseError};
use block_types::{builtin_descriptors, typecheck, ComponentDescriptor, ComponentPort, Diagnostic};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("lexical error: {0}")]
    Lex(#[from] LexError),
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    #[error("type check failed with {} diagnostic(s)", .0.len())]
    Type(Vec<Diagnostic>),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl CompileError {
    pub fn format_diagnostics(&self, filename: &str, source: &str) -> String {
        match self {
            CompileError::Lex(e) => format!("error[B001]: {e}\n  --> {filename}"),
            CompileError::Parse(e) => format!("error[B002]: {e}\n  --> {filename}"),
            CompileError::Type(diags) => diags
                .iter()
                .map(|d| d.format_with_source(filename, source))
                .collect::<Vec<_>>()
                .join("\n"),
            CompileError::Io(e) => format!("error[B003]: {e}\n  --> {filename}"),
        }
    }
}

pub fn compile_file(entry_path: &Path, project_root: &Path) -> Result<BlockIr, CompileError> {
    compile_file_with_packages(entry_path, project_root, &BTreeMap::new())
}

pub fn compile_file_with_packages(
    entry_path: &Path,
    project_root: &Path,
    packages: &BTreeMap<String, PathBuf>,
) -> Result<BlockIr, CompileError> {
    let canon_root = project_root.canonicalize()?;
    let canon_entry = entry_path.canonicalize()?;
    let mut visiting = HashSet::new();
    let mut modules = BTreeMap::new();
    compile_file_recursive(
        &canon_entry,
        &canon_root,
        packages,
        &mut visiting,
        &mut modules,
    )
}

fn compile_file_recursive(
    file_path: &Path,
    canon_root: &Path,
    packages: &BTreeMap<String, PathBuf>,
    visiting: &mut HashSet<PathBuf>,
    modules: &mut BTreeMap<String, BlockIr>,
) -> Result<BlockIr, CompileError> {
    if !file_path.starts_with(canon_root) {
        return Err(CompileError::Type(vec![Diagnostic {
            code: "B112".to_string(),
            message: format!("import path '{}' escapes project root", file_path.display()),
            line: 1,
            col: 1,
            span: None,
        }]));
    }
    if !visiting.insert(file_path.to_path_buf()) {
        return Err(CompileError::Type(vec![Diagnostic {
            code: "B111".to_string(),
            message: format!("circular import detected at '{}'", file_path.display()),
            line: 1,
            col: 1,
            span: None,
        }]));
    }

    let source = std::fs::read_to_string(file_path)?;
    let tokens = tokenize(&source)?;
    let ast = parse(&tokens)?;

    let mut registry = builtin_descriptors();
    let parent_dir = file_path.parent().unwrap_or(canon_root);

    for imp in &ast.imports {
        let (target_path, next_root) = if imp.path.starts_with('@') {
            let pkg_root = packages.get(&imp.path).ok_or_else(|| {
                CompileError::Type(vec![Diagnostic {
                    code: "B110".to_string(),
                    message: format!(
                        "package '{}' not found in Block.lock / local cache; run `sub install`",
                        imp.path
                    ),
                    line: imp.span.line,
                    col: imp.span.col,
                    span: Some(imp.span.clone()),
                }])
            })?;
            let canon_pkg_root = pkg_root.canonicalize().map_err(|_| {
                CompileError::Type(vec![Diagnostic {
                    code: "B110".to_string(),
                    message: format!("cached package directory missing for '{}'", imp.path),
                    line: imp.span.line,
                    col: imp.span.col,
                    span: Some(imp.span.clone()),
                }])
            })?;
            let pkg_manifest =
                block_package::PackageManifest::from_file(&canon_pkg_root.join("Block.toml"))
                    .map_err(|e| {
                        CompileError::Type(vec![Diagnostic {
                            code: "B110".to_string(),
                            message: format!("invalid Block.toml in package '{}': {e}", imp.path),
                            line: imp.span.line,
                            col: imp.span.col,
                            span: Some(imp.span.clone()),
                        }])
                    })?;
            let entry = canon_pkg_root
                .join(&pkg_manifest.package.entry)
                .canonicalize()
                .map_err(|_| {
                    CompileError::Type(vec![Diagnostic {
                        code: "B110".to_string(),
                        message: format!(
                            "missing entrypoint '{}' in package '{}'",
                            pkg_manifest.package.entry, imp.path
                        ),
                        line: imp.span.line,
                        col: imp.span.col,
                        span: Some(imp.span.clone()),
                    }])
                })?;
            (entry, canon_pkg_root)
        } else {
            let rel = Path::new(&imp.path);
            if rel.is_absolute() {
                return Err(CompileError::Type(vec![Diagnostic {
                    code: "B112".to_string(),
                    message: format!("absolute import path '{}' is not allowed", imp.path),
                    line: imp.span.line,
                    col: imp.span.col,
                    span: Some(imp.span.clone()),
                }]));
            }
            let entry = parent_dir.join(rel).canonicalize().map_err(|_| {
                CompileError::Type(vec![Diagnostic {
                    code: "B110".to_string(),
                    message: format!("missing import file '{}'", imp.path),
                    line: imp.span.line,
                    col: imp.span.col,
                    span: Some(imp.span.clone()),
                }])
            })?;
            (entry, canon_root.to_path_buf())
        };

        let mod_ir = compile_file_recursive(&target_path, &next_root, packages, visiting, modules)?;
        if mod_ir.block_name != imp.name {
            return Err(CompileError::Type(vec![Diagnostic {
                code: "B110".to_string(),
                message: format!(
                    "imported block '{}' not found in '{}' (found '{}')",
                    imp.name, imp.path, mod_ir.block_name
                ),
                line: imp.span.line,
                col: imp.span.col,
                span: Some(imp.span.clone()),
            }]));
        }
        if mod_ir.outputs.len() != 1 {
            return Err(CompileError::Type(vec![Diagnostic {
                code: "B109".to_string(),
                message: format!(
                    "imported block '{}' must have exactly 1 output to be used as a component (has {})",
                    imp.name,
                    mod_ir.outputs.len()
                ),
                line: imp.span.line,
                col: imp.span.col,
                span: Some(imp.span.clone()),
            }]));
        }

        let local_name = imp.local_name().to_string();
        let desc = ComponentDescriptor {
            id: local_name.clone(),
            version: "0.2.0".to_string(),
            inputs: mod_ir
                .inputs
                .iter()
                .map(|p| ComponentPort {
                    name: p.name.clone(),
                    ty: p.ty,
                })
                .collect(),
            output: mod_ir.outputs[0].ty,
            config_schema: vec![],
        };
        registry.insert(local_name.clone(), desc);
        modules.insert(local_name, mod_ir);
    }

    visiting.remove(file_path);

    let rel_name = file_path
        .strip_prefix(canon_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();
    let mut ir = compile_ast(&rel_name, &source, &ast, &registry)?;
    ir.modules = modules.clone();
    Ok(ir)
}

pub fn compile_source(source_file: &str, source: &str) -> Result<BlockIr, CompileError> {
    compile_source_with_registry(source_file, source, &builtin_descriptors())
}

pub fn compile_source_with_registry(
    source_file: &str,
    source: &str,
    registry: &HashMap<String, ComponentDescriptor>,
) -> Result<BlockIr, CompileError> {
    let tokens = tokenize(source)?;
    let ast = parse(&tokens)?;
    compile_ast(source_file, source, &ast, registry)
}

fn compile_ast(
    source_file: &str,
    source: &str,
    ast: &block_parser::BlockAst,
    registry: &HashMap<String, ComponentDescriptor>,
) -> Result<BlockIr, CompileError> {
    let typed = typecheck(ast, registry).map_err(CompileError::Type)?;

    let mut components = BTreeMap::new();
    for (alias, (desc, config)) in typed.instances {
        components.insert(
            alias,
            IrComponentInstance {
                component_id: desc.id,
                version: desc.version,
                config,
            },
        );
    }

    let nodes = typed
        .steps
        .into_iter()
        .enumerate()
        .map(|(id, step)| IrNode {
            id,
            target: step.target,
            output_ty: step.target_ty,
            instance_alias: step.alias,
            args: step.args,
        })
        .collect();

    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    let source_hash = hex::encode(hasher.finalize());

    Ok(BlockIr {
        schema_version: IR_SCHEMA_VERSION.to_string(),
        block_name: typed.name,
        inputs: typed
            .inputs
            .into_iter()
            .map(|(name, ty)| IrPort { name, ty })
            .collect(),
        outputs: typed
            .outputs
            .into_iter()
            .map(|(name, ty)| IrPort { name, ty })
            .collect(),
        components,
        nodes,
        modules: BTreeMap::new(),
        metadata: IrMetadata {
            source_file: source_file.to_string(),
            source_hash,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_and_validates_ir() {
        let src = r#"
block Greeting {
    input name: Text
    output message: Text
    use formatter = Format("Hello, {}")
    pipeline {
        message = formatter(name)
    }
}
"#;
        let ir = compile_source("main.block", src).unwrap();
        ir.validate().unwrap();
        let json = ir.to_deterministic_json().unwrap();
        let decoded = BlockIr::from_json(&json).unwrap();
        assert_eq!(ir, decoded);
    }
}
