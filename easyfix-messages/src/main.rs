use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use easyfix_messages::Generator;

#[derive(Parser)]
#[command(about = "Generate Rust types from FIX XML definitions")]
struct Cli {
    /// Path to FIXT transport XML (e.g., FIXT11.xml)
    #[arg(long)]
    fixt_xml: Option<PathBuf>,

    /// Path to FIX application XML (e.g., FIX50SP2.xml)
    #[arg(long)]
    fix_xml: PathBuf,

    /// Output path for the generated Rust source file
    #[arg(long)]
    output: PathBuf,

    /// Generate #[derive(serde::Serialize)] on generated types
    #[arg(long)]
    serde_serialize: bool,

    /// Generate #[derive(serde::Deserialize)] on generated types
    #[arg(long)]
    serde_deserialize: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut generator = Generator::new()
        .fix_xml(&cli.fix_xml)
        .output(&cli.output)
        .serde_serialize(cli.serde_serialize)
        .serde_deserialize(cli.serde_deserialize);

    if let Some(fixt_xml) = &cli.fixt_xml {
        generator = generator.fixt_xml(fixt_xml);
    }

    if let Err(err) = generator.generate() {
        eprintln!("Error: {err:#}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
