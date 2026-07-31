use anyhow::{Context, Result, bail};
use benchmark_core::{
    Corpus, compare_agent_sessions, evaluate_graph_jsonl, generate_medium_corpus,
    generate_rust_medium_corpus, generate_spring_medium_corpus, load_agent_session,
    validate_report, write_agent_comparison_report, write_graph_accuracy_report, write_report,
};
use std::env;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_lines)]
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
        "evaluate-graph" | "evaluate-java" => {
            expect_flag(&mut args, "--input")?;
            let input = resolve_output(&root, &args.next().context("--input requires a path")?);
            expect_flag(&mut args, "--truth")?;
            let truth = resolve_output(&root, &args.next().context("--truth requires a path")?);
            expect_flag(&mut args, "--output")?;
            let output = resolve_output(&root, &args.next().context("--output requires a path")?);
            expect_flag(&mut args, "--min-precision")?;
            let min_precision: f64 = args
                .next()
                .context("--min-precision requires a number")?
                .parse()
                .context("invalid precision threshold")?;
            expect_flag(&mut args, "--min-recall")?;
            let min_recall: f64 = args
                .next()
                .context("--min-recall requires a number")?
                .parse()
                .context("invalid recall threshold")?;
            reject_extra_args(args)?;
            let report = evaluate_graph_jsonl(&input, &truth)?;
            write_graph_accuracy_report(&output, &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.precision < min_precision || report.recall < min_recall {
                bail!(
                    "graph accuracy regression: precision {:.4} (min {:.4}), recall {:.4} (min {:.4})",
                    report.precision,
                    min_precision,
                    report.recall,
                    min_recall
                );
            }
        }
        "generate-medium" => {
            expect_flag(&mut args, "--output")?;
            let output = resolve_output(&root, &args.next().context("--output requires a path")?);
            expect_flag(&mut args, "--types")?;
            let types: usize = args
                .next()
                .context("--types requires a number")?
                .parse()
                .context("invalid type count")?;
            reject_extra_args(args)?;
            generate_medium_corpus(&output, types)?;
            println!("generated {types} types at {}", output.display());
        }
        "generate-spring-medium" => {
            generate_spring_medium_command(&mut args, &root)?;
        }
        "generate-rust-medium" => {
            expect_flag(&mut args, "--output")?;
            let output = resolve_output(&root, &args.next().context("--output requires a path")?);
            expect_flag(&mut args, "--modules")?;
            let modules: usize = args
                .next()
                .context("--modules requires a number")?
                .parse()
                .context("invalid module count")?;
            reject_extra_args(args)?;
            generate_rust_medium_corpus(&output, modules)?;
            println!("generated {modules} Rust modules at {}", output.display());
        }
        "compare-agents" => {
            expect_flag(&mut args, "--baseline")?;
            let baseline =
                resolve_output(&root, &args.next().context("--baseline requires a path")?);
            expect_flag(&mut args, "--graphine")?;
            let graphine =
                resolve_output(&root, &args.next().context("--graphine requires a path")?);
            expect_flag(&mut args, "--output")?;
            let output = resolve_output(&root, &args.next().context("--output requires a path")?);
            reject_extra_args(args)?;
            let corpus = Corpus::load(&root)?;
            let report = compare_agent_sessions(
                &root,
                &corpus,
                &load_agent_session(&baseline)?,
                &load_agent_session(&graphine)?,
            )?;
            write_agent_comparison_report(&output, &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => {
            eprintln!(
                "Graphine benchmark corpus tools\n\n  benchmark-core validate\n  benchmark-core stats\n  benchmark-core report --output <path>\n  benchmark-core evaluate-graph --input <jsonl> --truth <json> --output <json> --min-precision <n> --min-recall <n>\n  benchmark-core evaluate-java ...  # compatibility alias\n  benchmark-core compare-agents --baseline <session.json> --graphine <session.json> --output <report.json>\n  benchmark-core generate-medium --output <path> --types <n>\n  benchmark-core generate-spring-medium --output <path> --controllers <n>\n  benchmark-core generate-rust-medium --output <path> --modules <n>"
            );
            if command != "help" && command != "--help" && command != "-h" {
                bail!("unknown command: {command}");
            }
        }
    }
    Ok(())
}

fn generate_spring_medium_command(
    args: &mut impl Iterator<Item = String>,
    root: &Path,
) -> Result<()> {
    expect_flag(args, "--output")?;
    let output = resolve_output(root, &args.next().context("--output requires a path")?);
    expect_flag(args, "--controllers")?;
    let controllers: usize = args
        .next()
        .context("--controllers requires a number")?
        .parse()
        .context("invalid controller count")?;
    reject_extra_args(args)?;
    generate_spring_medium_corpus(&output, controllers)?;
    println!(
        "generated {controllers} Spring controller/service pairs at {}",
        output.display()
    );
    Ok(())
}

fn expect_flag(args: &mut impl Iterator<Item = String>, expected: &str) -> Result<()> {
    let actual = args
        .next()
        .with_context(|| format!("expected {expected}"))?;
    if actual != expected {
        bail!("expected {expected}, found {actual}");
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
