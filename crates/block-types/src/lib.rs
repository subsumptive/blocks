use block_lexer::Span;
use block_parser::{BlockAst, Literal, TypeName};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    Text(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Bytes(Vec<u8>),
    TokenSequence(Vec<String>),
}

impl Value {
    pub fn type_name(&self) -> TypeName {
        match self {
            Value::Text(_) => TypeName::Text,
            Value::Integer(_) => TypeName::Integer,
            Value::Float(_) => TypeName::Float,
            Value::Boolean(_) => TypeName::Boolean,
            Value::Bytes(_) => TypeName::Bytes,
            Value::TokenSequence(_) => TypeName::TokenSequence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentPort {
    pub name: String,
    pub ty: TypeName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigParamType {
    String,
    Integer,
    Float,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigParam {
    pub name: String,
    pub ty: ConfigParamType,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentDescriptor {
    pub id: String,
    pub version: String,
    pub inputs: Vec<ComponentPort>,
    pub output: TypeName,
    pub config_schema: Vec<ConfigParam>,
}

pub fn builtin_descriptors() -> HashMap<String, ComponentDescriptor> {
    let mut map = HashMap::new();
    map.insert(
        "Format".to_string(),
        ComponentDescriptor {
            id: "Format".to_string(),
            version: "0.1.0".to_string(),
            inputs: vec![ComponentPort {
                name: "value".to_string(),
                ty: TypeName::Text,
            }],
            output: TypeName::Text,
            config_schema: vec![ConfigParam {
                name: "template".to_string(),
                ty: ConfigParamType::String,
                required: true,
            }],
        },
    );
    map.insert(
        "Concat".to_string(),
        ComponentDescriptor {
            id: "Concat".to_string(),
            version: "0.1.0".to_string(),
            inputs: vec![
                ComponentPort {
                    name: "left".to_string(),
                    ty: TypeName::Text,
                },
                ComponentPort {
                    name: "right".to_string(),
                    ty: TypeName::Text,
                },
            ],
            output: TypeName::Text,
            config_schema: vec![],
        },
    );
    map.insert(
        "Tokenize".to_string(),
        ComponentDescriptor {
            id: "Tokenize".to_string(),
            version: "0.1.0".to_string(),
            inputs: vec![ComponentPort {
                name: "text".to_string(),
                ty: TypeName::Text,
            }],
            output: TypeName::TokenSequence,
            config_schema: vec![],
        },
    );
    map.insert(
        "CountTokens".to_string(),
        ComponentDescriptor {
            id: "CountTokens".to_string(),
            version: "0.1.0".to_string(),
            inputs: vec![ComponentPort {
                name: "tokens".to_string(),
                ty: TypeName::TokenSequence,
            }],
            output: TypeName::Integer,
            config_schema: vec![],
        },
    );
    map.insert(
        "Retrieve".to_string(),
        ComponentDescriptor {
            id: "Retrieve".to_string(),
            version: "0.1.0".to_string(),
            inputs: vec![ComponentPort {
                name: "query".to_string(),
                ty: TypeName::Text,
            }],
            output: TypeName::Text,
            config_schema: vec![ConfigParam {
                name: "source".to_string(),
                ty: ConfigParamType::String,
                required: true,
            }],
        },
    );
    map.insert(
        "Generate".to_string(),
        ComponentDescriptor {
            id: "Generate".to_string(),
            version: "0.1.0".to_string(),
            inputs: vec![
                ComponentPort {
                    name: "prompt".to_string(),
                    ty: TypeName::Text,
                },
                ComponentPort {
                    name: "context".to_string(),
                    ty: TypeName::Text,
                },
            ],
            output: TypeName::Text,
            config_schema: vec![ConfigParam {
                name: "backend".to_string(),
                ty: ConfigParamType::String,
                required: true,
            }],
        },
    );
    map
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[error("error[{code}]: {message} at {line}:{col}")]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub line: usize,
    pub col: usize,
    pub span: Option<Span>,
}

impl Diagnostic {
    pub fn format_with_source(&self, filename: &str, source: &str) -> String {
        let mut out = format!(
            "error[{}]: {}\n  --> {}:{}:{}\n",
            self.code, self.message, filename, self.line, self.col
        );
        if self.line > 0 {
            if let Some(line_str) = source.lines().nth(self.line - 1) {
                let line_num = self.line.to_string();
                let pad = " ".repeat(line_num.len());
                let caret_offset = self.col.saturating_sub(1);
                let caret_len = self
                    .span
                    .as_ref()
                    .map(|s| s.end.saturating_sub(s.start).max(1))
                    .unwrap_or(1);
                out.push_str(&format!(
                    "{pad} |\n{line_num} | {line_str}\n{pad} | {}{}\n",
                    " ".repeat(caret_offset),
                    "^".repeat(caret_len)
                ));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedStep {
    pub target: String,
    pub target_ty: TypeName,
    pub alias: String,
    pub component_id: String,
    pub component_version: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedBlock {
    pub name: String,
    pub inputs: Vec<(String, TypeName)>,
    pub outputs: Vec<(String, TypeName)>,
    pub instances: HashMap<String, (ComponentDescriptor, Vec<Literal>)>,
    pub steps: Vec<TypedStep>,
}

pub fn typecheck(
    ast: &BlockAst,
    registry: &HashMap<String, ComponentDescriptor>,
) -> Result<TypedBlock, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut env: HashMap<String, TypeName> = HashMap::new();
    let mut output_types: HashMap<String, TypeName> = HashMap::new();
    let mut instances: HashMap<String, (ComponentDescriptor, Vec<Literal>)> = HashMap::new();

    for inp in &ast.inputs {
        env.insert(inp.name.clone(), inp.ty);
    }
    for out in &ast.outputs {
        output_types.insert(out.name.clone(), out.ty);
    }

    for u in &ast.uses {
        let Some(desc) = registry.get(&u.component) else {
            diagnostics.push(Diagnostic {
                code: "B101".to_string(),
                message: format!("unknown component '{}'", u.component),
                line: u.span.line,
                col: u.span.col,
                span: Some(u.span.clone()),
            });
            continue;
        };

        let required_count = desc.config_schema.iter().filter(|c| c.required).count();
        if u.config.len() < required_count || u.config.len() > desc.config_schema.len() {
            diagnostics.push(Diagnostic {
                code: "B103".to_string(),
                message: format!(
                    "component '{}' expects {}..={} configuration arguments, found {}",
                    u.component,
                    required_count,
                    desc.config_schema.len(),
                    u.config.len()
                ),
                line: u.span.line,
                col: u.span.col,
                span: Some(u.span.clone()),
            });
            continue;
        }

        let mut config_ok = true;
        for (lit, param) in u.config.iter().zip(desc.config_schema.iter()) {
            let matches = matches!(
                (lit, &param.ty),
                (Literal::String(_), ConfigParamType::String)
                    | (Literal::Integer(_), ConfigParamType::Integer)
                    | (Literal::Float(_), ConfigParamType::Float)
                    | (Literal::Boolean(_), ConfigParamType::Boolean)
            );
            if !matches {
                diagnostics.push(Diagnostic {
                    code: "B103".to_string(),
                    message: format!(
                        "invalid configuration argument for '{}' in component '{}'",
                        param.name, u.component
                    ),
                    line: u.span.line,
                    col: u.span.col,
                    span: Some(u.span.clone()),
                });
                config_ok = false;
            }
        }
        if config_ok {
            instances.insert(u.alias.clone(), (desc.clone(), u.config.clone()));
        }
    }

    let mut assigned_outputs = HashSet::new();
    let mut steps = Vec::new();

    for stmt in &ast.pipeline {
        let Some((desc, _)) = instances.get(&stmt.callee).cloned() else {
            diagnostics.push(Diagnostic {
                code: "B104".to_string(),
                message: format!("unknown component instance '{}'", stmt.callee),
                line: stmt.callee_span.line,
                col: stmt.callee_span.col,
                span: Some(stmt.callee_span.clone()),
            });
            continue;
        };

        if stmt.args.len() != desc.inputs.len() {
            diagnostics.push(Diagnostic {
                code: "B105".to_string(),
                message: format!(
                    "component '{}' expects {} argument(s), found {}",
                    desc.id,
                    desc.inputs.len(),
                    stmt.args.len()
                ),
                line: stmt.callee_span.line,
                col: stmt.callee_span.col,
                span: Some(stmt.callee_span.clone()),
            });
            continue;
        }

        let mut arg_names = Vec::new();
        for (arg, expected_port) in stmt.args.iter().zip(desc.inputs.iter()) {
            match env.get(&arg.name) {
                None => {
                    diagnostics.push(Diagnostic {
                        code: "B106".to_string(),
                        message: format!("unknown identifier '{}'", arg.name),
                        line: arg.span.line,
                        col: arg.span.col,
                        span: Some(arg.span.clone()),
                    });
                }
                Some(actual_ty) if *actual_ty != expected_port.ty => {
                    diagnostics.push(Diagnostic {
                        code: "B102".to_string(),
                        message: format!(
                            "incompatible types: expected {}, found {}",
                            expected_port.ty, actual_ty
                        ),
                        line: arg.span.line,
                        col: arg.span.col,
                        span: Some(arg.span.clone()),
                    });
                }
                Some(_) => {
                    arg_names.push(arg.name.clone());
                }
            }
        }

        if let Some(expected_out_ty) = output_types.get(&stmt.target) {
            if *expected_out_ty != desc.output {
                diagnostics.push(Diagnostic {
                    code: "B102".to_string(),
                    message: format!(
                        "incompatible output type for '{}': expected {}, found {}",
                        stmt.target, expected_out_ty, desc.output
                    ),
                    line: stmt.target_span.line,
                    col: stmt.target_span.col,
                    span: Some(stmt.target_span.clone()),
                });
            }
            assigned_outputs.insert(stmt.target.clone());
        } else if env.contains_key(&stmt.target) {
            diagnostics.push(Diagnostic {
                code: "B107".to_string(),
                message: format!("duplicate variable assignment '{}'", stmt.target),
                line: stmt.target_span.line,
                col: stmt.target_span.col,
                span: Some(stmt.target_span.clone()),
            });
        }

        env.insert(stmt.target.clone(), desc.output);
        steps.push(TypedStep {
            target: stmt.target.clone(),
            target_ty: desc.output,
            alias: stmt.callee.clone(),
            component_id: desc.id.clone(),
            component_version: desc.version.clone(),
            args: arg_names,
        });
    }

    for out in &ast.outputs {
        if !assigned_outputs.contains(&out.name) {
            diagnostics.push(Diagnostic {
                code: "B108".to_string(),
                message: format!("output '{}' is never assigned in pipeline", out.name),
                line: out.span.line,
                col: out.span.col,
                span: Some(out.span.clone()),
            });
        }
    }

    if diagnostics.is_empty() {
        Ok(TypedBlock {
            name: ast.name.clone(),
            inputs: ast.inputs.iter().map(|i| (i.name.clone(), i.ty)).collect(),
            outputs: ast.outputs.iter().map(|o| (o.name.clone(), o.ty)).collect(),
            instances,
            steps,
        })
    } else {
        Err(diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use block_lexer::tokenize;
    use block_parser::parse;

    #[test]
    fn typechecks_valid_pipeline() {
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
        let ast = parse(&tokenize(src).unwrap()).unwrap();
        let typed = typecheck(&ast, &builtin_descriptors()).unwrap();
        assert_eq!(typed.steps.len(), 1);
    }

    #[test]
    fn rejects_incompatible_types() {
        let src = r#"
block Bad {
    input query: Text
    output count: Integer
    use counter = CountTokens()
    pipeline {
        count = counter(query)
    }
}
"#;
        let ast = parse(&tokenize(src).unwrap()).unwrap();
        let diags = typecheck(&ast, &builtin_descriptors()).unwrap_err();
        assert_eq!(diags[0].code, "B102");
    }

    #[test]
    fn rejects_missing_output_assignment() {
        let src = r#"
block MissingOut {
    input query: Text
    output answer: Text
    use fmt = Format("{}")
    pipeline {
        tmp = fmt(query)
    }
}
"#;
        let ast = parse(&tokenize(src).unwrap()).unwrap();
        let diags = typecheck(&ast, &builtin_descriptors()).unwrap_err();
        assert!(diags.iter().any(|d| d.code == "B108"));
    }
}
