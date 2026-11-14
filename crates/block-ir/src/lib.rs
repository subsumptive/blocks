use block_parser::{Literal, TypeName};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;

pub const IR_SCHEMA_VERSION: &str = "0.2.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockIr {
    pub schema_version: String,
    pub block_name: String,
    pub inputs: Vec<IrPort>,
    pub outputs: Vec<IrPort>,
    pub components: BTreeMap<String, IrComponentInstance>,
    pub nodes: Vec<IrNode>,
    #[serde(default)]
    pub modules: BTreeMap<String, BlockIr>,
    pub metadata: IrMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrPort {
    pub name: String,
    pub ty: TypeName,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IrComponentInstance {
    pub component_id: String,
    pub version: String,
    pub config: Vec<Literal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrNode {
    pub id: usize,
    pub target: String,
    pub output_ty: TypeName,
    pub instance_alias: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrMetadata {
    pub source_file: String,
    pub source_hash: String,
}

#[derive(Debug, Error)]
pub enum IrValidationError {
    #[error("unsupported IR schema version '{found}', expected '{expected}'")]
    UnsupportedSchemaVersion { found: String, expected: String },
    #[error("empty block name in IR")]
    EmptyBlockName,
    #[error("node {node_id} references unknown component instance '{alias}'")]
    UnknownInstance { node_id: usize, alias: String },
    #[error("node {node_id} references undefined variable '{var}'")]
    UndefinedVariable { node_id: usize, var: String },
    #[error("declared output '{name}' is not produced by any node")]
    MissingOutput { name: String },
}

impl BlockIr {
    pub fn to_deterministic_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn validate(&self) -> Result<(), IrValidationError> {
        if self.schema_version != "0.1.0" && self.schema_version != IR_SCHEMA_VERSION {
            return Err(IrValidationError::UnsupportedSchemaVersion {
                found: self.schema_version.clone(),
                expected: IR_SCHEMA_VERSION.to_string(),
            });
        }
        for module in self.modules.values() {
            module.validate()?;
        }
        if self.block_name.trim().is_empty() {
            return Err(IrValidationError::EmptyBlockName);
        }

        let mut defined: HashSet<String> = self.inputs.iter().map(|i| i.name.clone()).collect();
        for node in &self.nodes {
            if !self.components.contains_key(&node.instance_alias) {
                return Err(IrValidationError::UnknownInstance {
                    node_id: node.id,
                    alias: node.instance_alias.clone(),
                });
            }
            for arg in &node.args {
                if !defined.contains(arg) {
                    return Err(IrValidationError::UndefinedVariable {
                        node_id: node.id,
                        var: arg.clone(),
                    });
                }
            }
            defined.insert(node.target.clone());
        }

        for out in &self.outputs {
            if !defined.contains(&out.name) {
                return Err(IrValidationError::MissingOutput {
                    name: out.name.clone(),
                });
            }
        }
        Ok(())
    }
}
