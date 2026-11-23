#![forbid(unsafe_code)]

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "Usage: cargo run -p xtask -- gen-pki [DIRECTORY]\n\nGenerate fresh Brainpool test identities for the HAN and WAN proxy legs.\nThe output directory must be new. Default: target/test-pki";

fn run() -> xtask::Result<()> {
    let mut args = env::args_os().skip(1);
    match args.next().as_deref().and_then(|s| s.to_str()) {
        Some("--help" | "-h") => println!("{HELP}"),
        Some("gen-pki") => {
            let path = args
                .next()
                .map_or_else(|| PathBuf::from("target/test-pki"), PathBuf::from);
            if args.next().is_some() {
                return Err(HELP.into());
            }
            if path == std::path::Path::new("--help") || path == std::path::Path::new("-h") {
                println!("{HELP}");
                return Ok(());
            }
            xtask::generate(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("Generated HAN and WAN fixtures in {}", path.display());
        }
        _ => return Err(HELP.into()),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}
