use anyhow::{anyhow, Result};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub type Task = Map<String, Value>;

#[derive(Clone, Debug)]
pub struct Program {
    pub default_state: Map<String, Value>,
    pub order: Vec<Task>,
    pub named_tasks: HashMap<String, Task>,
    pub save_states: Map<String, Value>,
    pub completion_args: Map<String, Value>,
}

#[derive(Clone, Debug)]
pub struct ProgramLoadContext {
    pub program_path: PathBuf,
    pub program_dir: PathBuf,
    pub inserts_dir: Option<PathBuf>,
}

impl ProgramLoadContext {
    pub fn new(program_path: PathBuf, inserts_dir: Option<PathBuf>) -> Result<Self> {
        let program_dir = program_path
            .parent()
            .ok_or_else(|| anyhow!("Program path has no parent directory"))?
            .to_path_buf();
        if let Some(dir) = inserts_dir.as_ref() {
            if !dir.is_dir() {
                return Err(anyhow!(
                    "--inserts-dir must be an existing directory, got '{}'",
                    dir.display()
                ));
            }
        };
        Ok(Self {
            program_path,
            program_dir,
            inserts_dir,
        })
    }
}
