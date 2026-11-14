use block_ir::{BlockIr, IrValidationError};
use block_parser::{Literal, TypeName};
use block_types::{builtin_descriptors, ComponentDescriptor, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("IR validation failed: {0}")]
    InvalidIr(#[from] IrValidationError),
    #[error("missing required input '{name}'")]
    MissingInput { name: String },
    #[error("input '{name}' type mismatch: expected {expected}, found {found}")]
    InputTypeMismatch {
        name: String,
        expected: TypeName,
        found: TypeName,
    },
    #[error("unknown component '{id}'")]
    UnknownComponent { id: String },
    #[error("component '{id}' version mismatch: IR requires {required}, runtime has {available}")]
    VersionMismatch {
        id: String,
        required: String,
        available: String,
    },
    #[error("component '{id}' execution error: {message}")]
    ComponentFailed { id: String, message: String },
    #[error("path traversal or invalid path '{path}'")]
    InvalidPath { path: String },
    #[error("maximum Block invocation depth ({max_depth}) exceeded")]
    MaxDepthExceeded { max_depth: usize },
}

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub project_root: PathBuf,
    pub max_depth: usize,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            project_root: PathBuf::from("."),
            max_depth: 32,
        }
    }
}

pub trait Component: Send + Sync {
    fn descriptor(&self) -> &ComponentDescriptor;

    fn execute(
        &self,
        config: &[Literal],
        inputs: &[Value],
        context: &ExecutionContext,
    ) -> Result<Value, RuntimeError>;
}

pub struct ComponentRegistry {
    components: HashMap<String, Arc<dyn Component>>,
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        let mut reg = Self {
            components: HashMap::new(),
        };
        let descs = builtin_descriptors();
        reg.register(Arc::new(FormatComponent(descs["Format"].clone())));
        reg.register(Arc::new(ConcatComponent(descs["Concat"].clone())));
        reg.register(Arc::new(TokenizeComponent(descs["Tokenize"].clone())));
        reg.register(Arc::new(CountTokensComponent(descs["CountTokens"].clone())));
        reg.register(Arc::new(RetrieveComponent(descs["Retrieve"].clone())));
        reg.register(Arc::new(GenerateComponent(descs["Generate"].clone())));
        reg
    }
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, component: Arc<dyn Component>) {
        self.components
            .insert(component.descriptor().id.clone(), component);
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn Component>> {
        self.components.get(id)
    }

    pub fn descriptors(&self) -> HashMap<String, ComponentDescriptor> {
        self.components
            .iter()
            .map(|(k, v)| (k.clone(), v.descriptor().clone()))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct NodeMeasurement {
    pub node_id: usize,
    pub target: String,
    pub component_id: String,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct ExecutionReport {
    pub outputs: BTreeMap<String, Value>,
    pub node_measurements: Vec<NodeMeasurement>,
    pub total_duration: Duration,
}

pub fn execute(
    ir: &BlockIr,
    inputs: &BTreeMap<String, Value>,
    registry: &ComponentRegistry,
    context: &ExecutionContext,
) -> Result<ExecutionReport, RuntimeError> {
    ir.validate()?;
    execute_with_depth(ir, inputs, registry, context, &ir.modules, 0)
}

fn execute_with_depth(
    ir: &BlockIr,
    inputs: &BTreeMap<String, Value>,
    registry: &ComponentRegistry,
    context: &ExecutionContext,
    modules: &BTreeMap<String, BlockIr>,
    depth: usize,
) -> Result<ExecutionReport, RuntimeError> {
    if depth > context.max_depth {
        return Err(RuntimeError::MaxDepthExceeded {
            max_depth: context.max_depth,
        });
    }
    let start_all = Instant::now();

    let mut env: HashMap<String, Value> = HashMap::new();
    for port in &ir.inputs {
        let val = inputs
            .get(&port.name)
            .ok_or_else(|| RuntimeError::MissingInput {
                name: port.name.clone(),
            })?;
        if val.type_name() != port.ty {
            return Err(RuntimeError::InputTypeMismatch {
                name: port.name.clone(),
                expected: port.ty,
                found: val.type_name(),
            });
        }
        env.insert(port.name.clone(), val.clone());
    }

    let mut node_measurements = Vec::with_capacity(ir.nodes.len());

    for node in &ir.nodes {
        let inst = &ir.components[&node.instance_alias];

        let mut arg_vals = Vec::with_capacity(node.args.len());
        for arg_name in &node.args {
            let val = env
                .get(arg_name)
                .ok_or_else(|| RuntimeError::ComponentFailed {
                    id: inst.component_id.clone(),
                    message: format!("missing value for argument '{arg_name}'"),
                })?;
            arg_vals.push(val.clone());
        }

        let t0 = Instant::now();
        let result = if let Some(sub_ir) = modules.get(&inst.component_id) {
            let mut sub_inputs = BTreeMap::new();
            for (port, val) in sub_ir.inputs.iter().zip(arg_vals) {
                sub_inputs.insert(port.name.clone(), val);
            }
            let sub_report =
                execute_with_depth(sub_ir, &sub_inputs, registry, context, modules, depth + 1)?;
            let out_port = sub_ir
                .outputs
                .first()
                .ok_or_else(|| RuntimeError::ComponentFailed {
                    id: inst.component_id.clone(),
                    message: "imported block has no outputs".to_string(),
                })?;
            sub_report
                .outputs
                .get(&out_port.name)
                .cloned()
                .ok_or_else(|| RuntimeError::ComponentFailed {
                    id: inst.component_id.clone(),
                    message: format!("missing output '{}'", out_port.name),
                })?
        } else {
            let comp =
                registry
                    .get(&inst.component_id)
                    .ok_or_else(|| RuntimeError::UnknownComponent {
                        id: inst.component_id.clone(),
                    })?;

            if comp.descriptor().version != inst.version {
                return Err(RuntimeError::VersionMismatch {
                    id: inst.component_id.clone(),
                    required: inst.version.clone(),
                    available: comp.descriptor().version.clone(),
                });
            }

            comp.execute(&inst.config, &arg_vals, context)?
        };
        let elapsed = t0.elapsed();

        env.insert(node.target.clone(), result);
        node_measurements.push(NodeMeasurement {
            node_id: node.id,
            target: node.target.clone(),
            component_id: inst.component_id.clone(),
            duration: elapsed,
        });
    }

    let mut outputs = BTreeMap::new();
    for out in &ir.outputs {
        if let Some(val) = env.remove(&out.name) {
            outputs.insert(out.name.clone(), val);
        }
    }

    Ok(ExecutionReport {
        outputs,
        node_measurements,
        total_duration: start_all.elapsed(),
    })
}

struct FormatComponent(ComponentDescriptor);
impl Component for FormatComponent {
    fn descriptor(&self) -> &ComponentDescriptor {
        &self.0
    }
    fn execute(
        &self,
        config: &[Literal],
        inputs: &[Value],
        _context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let template = match config.first() {
            Some(Literal::String(s)) => s,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected string template".to_string(),
                })
            }
        };
        let input = match inputs.first() {
            Some(Value::Text(s)) => s,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected Text input".to_string(),
                })
            }
        };
        Ok(Value::Text(template.replacen("{}", input, 1)))
    }
}

struct ConcatComponent(ComponentDescriptor);
impl Component for ConcatComponent {
    fn descriptor(&self) -> &ComponentDescriptor {
        &self.0
    }
    fn execute(
        &self,
        _config: &[Literal],
        inputs: &[Value],
        _context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        match (inputs.first(), inputs.get(1)) {
            (Some(Value::Text(a)), Some(Value::Text(b))) => Ok(Value::Text(format!("{a}{b}"))),
            _ => Err(RuntimeError::ComponentFailed {
                id: self.0.id.clone(),
                message: "expected two Text inputs".to_string(),
            }),
        }
    }
}

struct TokenizeComponent(ComponentDescriptor);
impl Component for TokenizeComponent {
    fn descriptor(&self) -> &ComponentDescriptor {
        &self.0
    }
    fn execute(
        &self,
        _config: &[Literal],
        inputs: &[Value],
        _context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let text = match inputs.first() {
            Some(Value::Text(s)) => s,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected Text input".to_string(),
                })
            }
        };
        // Deterministic baseline whitespace + punctuation tokenizer for v0.1
        let tokens: Vec<String> = text
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect();
        Ok(Value::TokenSequence(tokens))
    }
}

struct CountTokensComponent(ComponentDescriptor);
impl Component for CountTokensComponent {
    fn descriptor(&self) -> &ComponentDescriptor {
        &self.0
    }
    fn execute(
        &self,
        _config: &[Literal],
        inputs: &[Value],
        _context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let tokens = match inputs.first() {
            Some(Value::TokenSequence(t)) => t,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected TokenSequence input".to_string(),
                })
            }
        };
        Ok(Value::Integer(tokens.len() as i64))
    }
}

struct RetrieveComponent(ComponentDescriptor);
impl Component for RetrieveComponent {
    fn descriptor(&self) -> &ComponentDescriptor {
        &self.0
    }
    fn execute(
        &self,
        config: &[Literal],
        inputs: &[Value],
        context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let source = match config.first() {
            Some(Literal::String(s)) => s,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected corpus source string".to_string(),
                })
            }
        };
        let query = match inputs.first() {
            Some(Value::Text(q)) => q,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected Text query".to_string(),
                })
            }
        };

        let corpus_text = if source == "local" {
            "Subsumptive Blocks is a declarative language for composable AI systems.\n\
             Components are strongly typed and connected in deterministic pipelines.\n\
             Intelligence, unbundled."
                .to_string()
        } else {
            let rel = Path::new(source);
            if rel.is_absolute()
                || rel
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(RuntimeError::InvalidPath {
                    path: source.clone(),
                });
            }
            let full = context.project_root.join(rel);
            std::fs::read_to_string(&full).map_err(|e| RuntimeError::ComponentFailed {
                id: self.0.id.clone(),
                message: format!("failed to read corpus '{}': {e}", full.display()),
            })?
        };

        let query_terms: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        let mut best_line = "";
        let mut best_score = 0usize;
        for line in corpus_text.lines() {
            let lower = line.to_lowercase();
            let score = query_terms
                .iter()
                .filter(|t| lower.contains(t.as_str()))
                .count();
            if score > best_score {
                best_score = score;
                best_line = line;
            }
        }
        if best_line.is_empty() {
            best_line = corpus_text.lines().next().unwrap_or("");
        }
        Ok(Value::Text(best_line.to_string()))
    }
}

struct GenerateComponent(ComponentDescriptor);
impl Component for GenerateComponent {
    fn descriptor(&self) -> &ComponentDescriptor {
        &self.0
    }
    fn execute(
        &self,
        config: &[Literal],
        inputs: &[Value],
        _context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let backend = match config.first() {
            Some(Literal::String(s)) => s,
            _ => "mock",
        };
        let prompt = match inputs.first() {
            Some(Value::Text(p)) => p,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected Text prompt".to_string(),
                })
            }
        };
        let context = match inputs.get(1) {
            Some(Value::Text(c)) => c,
            _ => {
                return Err(RuntimeError::ComponentFailed {
                    id: self.0.id.clone(),
                    message: "expected Text context".to_string(),
                })
            }
        };
        // Deterministic mock generator for v0.1 pipeline verification
        Ok(Value::Text(format!(
            "[mock:{backend}] query=\"{prompt}\" | context=\"{context}\""
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use block_compiler::compile_source;

    #[test]
    fn executes_greeting_pipeline() {
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
        let mut inputs = BTreeMap::new();
        inputs.insert("name".to_string(), Value::Text("World".to_string()));
        let report = execute(
            &ir,
            &inputs,
            &ComponentRegistry::default(),
            &ExecutionContext::default(),
        )
        .unwrap();
        assert_eq!(
            report.outputs.get("message"),
            Some(&Value::Text("Hello, World".to_string()))
        );
    }
}
