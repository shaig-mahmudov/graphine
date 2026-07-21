use anyhow::{Context, Result, bail};
use benchmark_core::{Corpus, validate_report, write_report};
use std::env;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_owned());
    let root = workspace_root()?;
    match command.as_str() {
        "validate" => {
            reject_extra_args(args)?;
            let corpus = Corpus::load(&root)?;
            println!(
                "valid: {} questions and {} ground-truth answers",
                corpus.questions.len(),
                corpus.ground_truth.len()
            );
        }
        "stats" => {
            reject_extra_args(args)?;
            let corpus = Corpus::load(&root)?;
            println!("{}", serde_json::to_string_pretty(&corpus.stats())?);
        }
        "report" => {
            let flag = args.next().context("report requires --output <path>")?;
            if flag != "--output" {
                bail!("expected --output, found {flag}");
            }
            let output = args.next().context("--output requires a path")?;
            reject_extra_args(args)?;
            let corpus = Corpus::load(&root)?;
            let report = corpus.report();
            validate_report(&root, &report)?;
            let path = resolve_output(&root, &output);
            write_report(&path, &report)?;
            println!("wrote {}", path.display());
        }
        _ => {
            eprintln!(
                "Graphine benchmark corpus tools\n\n  benchmark-core validate\n  benchmark-core stats\n  benchmark-core report --output <path>"
            );
            if command != "help" && command != "--help" && command != "-h" {
                bail!("unknown command: {command}");
            }
        }
    }
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("benchmark-core must live at crates/benchmark-core")
}

fn resolve_output(root: &Path, output: &str) -> PathBuf {
    let path = Path::new(output);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn reject_extra_args(mut args: impl Iterator<Item = String>) -> Result<()> {
    if let Some(extra) = args.next() {
        bail!("unexpected argument: {extra}");
    }
    Ok(())
}
