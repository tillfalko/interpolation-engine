mod analyzer;
mod chat;
mod filter;
mod interp;
mod math;
mod model;
mod parser;
mod runtime;
mod save;
mod ui;

use anyhow::Result;
use clap::Parser;
use model::{Program, ProgramLoadContext};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "interpolation_engine")]
#[command(about = "Run an interpolation-engine program.", long_about = None)]
struct Args {
    /// Path to the .json5 program file.
    program: Option<PathBuf>,
    /// Extra positional arguments passed to the program and accessible via '{ARG1}', '{ARG2}', etc.
    #[arg(last = true)]
    program_arguments: Vec<String>,
    /// Specify a path to store log info at.
    #[arg(long)]
    log: Option<PathBuf>,
    /// Path to store input history at. (Reserved for future use)
    #[arg(long)]
    history: Option<PathBuf>,
    /// Optional directory to load inserts from when a key is not found in state['inserts'].
    #[arg(long = "inserts-dir")]
    inserts_dir: Option<PathBuf>,
    /// Enable agent mode (file-based interaction).
    #[arg(long = "agent-mode")]
    agent_mode: bool,
    /// Agent output path (JSON payload).
    #[arg(long = "agent-output", default_value = "/tmp/agent_output")]
    agent_output: PathBuf,
    /// Agent input path (selected choice / text).
    #[arg(long = "agent-input", default_value = "/tmp/agent_input")]
    agent_input: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.program.is_none() {
        eprintln!("Error: specify a program (.json5 file) to run.");
        return Ok(());
    }

    let program_path = args.program.unwrap();
    let inserts_dir = args.inserts_dir.clone();

    let mut load_ctx = ProgramLoadContext::new(program_path.clone(), inserts_dir.clone())?;
    let mut program: Program = parser::load_program(&mut load_ctx)?;

    analyzer::analyze_program(&program, &load_ctx)?;

    runtime::run_program(
        &mut program,
        &load_ctx,
        &args.program_arguments,
        runtime::RuntimeOptions {
            agent_mode: args.agent_mode,
            agent_input: args.agent_input,
            agent_output: args.agent_output,
            log_path: args.log,
            history_path: args.history,
        },
    )
    .await?;

    Ok(())
}
