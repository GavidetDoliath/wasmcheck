use clap::{Parser, ValueEnum};
use owo_colors::OwoColorize;
use std::process;

use wasmcheck::{SizeReport, WasmCheckError, find_wasm_files, format_size, parse_size};

#[derive(Parser)]
#[command(name = "wasmcheck", about = "WASM bundle size budget checker")]
struct Cli {
    /// Path to a specific .wasm file
    #[arg(short, long)]
    file: Option<String>,

    /// Size budget (e.g. "280 KB", "2.5 MB")
    #[arg(short, long)]
    budget: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,

    /// Show top N heaviest functions (requires name section, best-effort)
    #[arg(long)]
    top: Option<usize>,
}

#[derive(Clone, ValueEnum)]
enum Format {
    Table,
    Json,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{} {e}", "error:".red().bold());
        process::exit(1);
    }
}

fn run() -> Result<(), WasmCheckError> {
    let cli = Cli::parse();

    let path = match &cli.file {
        Some(p) => {
            if !std::path::Path::new(p).exists() {
                return Err(WasmCheckError::FileNotFound(p.clone()));
            }
            p.clone()
        }
        None => detect_wasm_file()?,
    };

    let report = SizeReport::measure(&path)?;

    match cli.format {
        Format::Json => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }
        Format::Table => print_table(&report),
    }

    if let Some(budget_str) = &cli.budget {
        let budget = parse_size(budget_str).map_err(|e| {
            WasmCheckError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
        })?;

        if report.raw > budget {
            eprintln!(
                "\n{} raw {} exceeds budget {}",
                "FAIL".red().bold(),
                format_size(report.raw).red(),
                format_size(budget).yellow()
            );
            process::exit(1);
        } else {
            eprintln!(
                "\n{} raw {} within budget {}",
                "PASS".green().bold(),
                format_size(report.raw).green(),
                format_size(budget).yellow()
            );
        }
    }

    Ok(())
}

fn detect_wasm_file() -> Result<String, WasmCheckError> {
    let cwd = std::env::current_dir()
        .map_err(WasmCheckError::Io)?
        .to_string_lossy()
        .to_string();

    let files = find_wasm_files(&cwd)?;

    match files.len() {
        0 => Err(WasmCheckError::NoWasmFound),
        1 => Ok(files.into_iter().next().unwrap()),
        _ => Err(WasmCheckError::MultipleWasmFound { found: files }),
    }
}

fn print_table(report: &SizeReport) {
    println!();
    println!("  {} {}", "file:".dimmed(), report.file);
    println!("  {:<10} {:>12}", "raw".dimmed(), format_size(report.raw));
    println!("  {:<10} {:>12}", "gzip".dimmed(), format_size(report.gzip));
    println!(
        "  {:<10} {:>12}",
        "brotli".dimmed(),
        format_size(report.brotli)
    );
    println!();
}
