use clap::{Args, Parser, Subcommand, ValueEnum};
use owo_colors::OwoColorize;
use std::process;

use wasmcheck::{
    Budget, CONFIG_FILE, Config, Metric, SizeReport, WasmCheckError, delta_str, find_wasm_files,
    format_size, top_functions,
};

#[derive(Parser)]
#[command(name = "wasmcheck", version, about = "WASM bundle size budget checker")]
struct Cli {
    /// Path to a specific .wasm file
    #[arg(short, long)]
    file: Option<String>,

    /// Size budget (e.g. "280 KB", "2.5 MB") — applies to raw size
    #[arg(short, long)]
    budget: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,

    /// Show top N heaviest functions (requires name section, best-effort)
    #[arg(long)]
    top: Option<usize>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Check .wasm bundle sizes against budget+baseline
    Check(CheckArgs),
    /// Create a .wasmcheck.json config with current sizes as baseline
    Init(InitArgs),
    /// Refresh the baseline in .wasmcheck.json to current sizes
    Baseline(InitArgs),
}

#[derive(Args)]
struct CheckArgs {
    /// Path to a specific .wasm file
    #[arg(short, long)]
    file: Option<String>,

    /// Size budget (e.g. "280 KB", "2.5 MB") — applies to raw size
    #[arg(short, long)]
    budget: Option<String>,

    /// Path to config file (default: ./.wasmcheck.json if present)
    #[arg(short, long)]
    config: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Table)]
    format: Format,

    /// Show top N heaviest functions (requires name section, best-effort)
    #[arg(long)]
    top: Option<usize>,
}

#[derive(Args)]
struct InitArgs {
    /// Path to config file to write (default: ./.wasmcheck.json)
    #[arg(short, long)]
    config: Option<String>,

    /// Path to a specific .wasm file
    #[arg(short, long)]
    file: Option<String>,
}

#[derive(Clone, ValueEnum)]
enum Format {
    Table,
    Json,
}

fn main() {
    match run() {
        Ok(()) => process::exit(0),
        Err(e) => {
            if e.is_budget_exceeded() {
                eprintln!("{} {e}", "FAIL".red().bold());
                process::exit(1);
            }
            eprintln!("{} {e}", "error:".red().bold());
            process::exit(1);
        }
    }
}

fn run() -> Result<(), WasmCheckError> {
    let cli = Cli::parse();

    match cli.command {
        None => run_check(CheckArgs {
            file: cli.file,
            budget: cli.budget,
            config: None,
            format: cli.format,
            top: cli.top,
        }),
        Some(Commands::Check(args)) => run_check(args),
        Some(Commands::Init(args)) => run_init(&args, false),
        Some(Commands::Baseline(args)) => run_init(&args, true),
    }
}

fn config_path(explicit: &Option<String>) -> String {
    explicit.clone().unwrap_or_else(|| CONFIG_FILE.to_string())
}

fn resolve_files(
    cli_file: &Option<String>,
    config: Option<&Config>,
) -> Result<Vec<String>, WasmCheckError> {
    if let Some(file) = cli_file {
        if !std::path::Path::new(file).exists() {
            return Err(WasmCheckError::FileNotFound(file.clone()));
        }
        return Ok(vec![file.clone()]);
    }

    if let Some(cfg) = config
        && !cfg.files.is_empty()
    {
        let mut resolved: Vec<String> = Vec::new();
        for entry in &cfg.files {
            if entry.contains(['*', '?', '[']) {
                let mut matches: Vec<String> = glob::glob(entry)
                    .map_err(|e| WasmCheckError::Io(std::io::Error::other(e.to_string())))?
                    .filter_map(|p| p.ok())
                    .filter(|p| p.is_file())
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                matches.sort();
                resolved.extend(matches);
            } else {
                resolved.push(entry.clone());
            }
        }
        resolved.sort();
        resolved.dedup();
        if resolved.is_empty() {
            return Err(WasmCheckError::NoWasmFound);
        }
        return Ok(resolved);
    }

    // auto-detect in current directory
    let cwd = std::env::current_dir().map_err(WasmCheckError::Io)?;
    let files = find_wasm_files(&cwd.to_string_lossy())?;

    match files.len() {
        0 => Err(WasmCheckError::NoWasmFound),
        1 => Ok(files),
        _ => Err(WasmCheckError::MultipleWasmFound { found: files }),
    }
}

fn run_check(args: CheckArgs) -> Result<(), WasmCheckError> {
    let cfg_path = config_path(&args.config);

    // config is optional; auto-discover
    let config: Option<Config> = if std::path::Path::new(&cfg_path).exists() {
        Some(Config::load(&cfg_path)?)
    } else if args.config.is_some() {
        return Err(WasmCheckError::ConfigNotFound(cfg_path));
    } else {
        None
    };

    // Merge: CLI --budget overrides config budget's raw; else config budget.
    let config_budget = config.as_ref().and_then(|c| c.budget.clone());
    let effective_budget: Option<Budget> = match (&args.budget, config_budget) {
        (Some(raw), None) => Some(Budget::from_raw(raw.clone())?),
        (Some(raw), Some(mut cfg)) => {
            cfg.raw = Some(raw.clone());
            Some(cfg)
        }
        (None, Some(cfg)) => Some(cfg),
        (None, None) => None,
    };

    let files = resolve_files(&args.file, config.as_ref())?;

    let mut any_failed = false;

    for file in &files {
        let report = SizeReport::measure(file)?;
        let baseline = config.as_ref().and_then(|c| c.baseline.get(file));

        match args.format {
            Format::Json => {
                let mut obj = serde_json::to_value(&report)?;
                if let Some(base) = baseline {
                    obj["baseline"] = serde_json::to_value(base)?;
                }
                println!("{}", serde_json::to_string_pretty(&obj)?);
            }
            Format::Table => {
                print_table(&report, baseline, effective_budget.as_ref())?;
                if let Some(top) = args.top {
                    print_top(file, top)?;
                }
            }
        }

        if let Some(budget) = &effective_budget {
            let exceeded = budget.exceeded_metrics(&report)?;
            if !exceeded.is_empty() {
                any_failed = true;
                let what: Vec<&str> = exceeded.iter().map(Metric::label).collect();
                eprintln!(
                    "\n{} {} exceeds budget: {}",
                    "FAIL".red().bold(),
                    file.cyan(),
                    what.join(", ")
                );
            } else {
                eprintln!("\n{} {}", "PASS".green().bold(), file.cyan());
            }
        }
    }

    if any_failed {
        return Err(WasmCheckError::BudgetExceeded(
            "one or more files exceed the budget".into(),
        ));
    }

    Ok(())
}

fn run_init(args: &InitArgs, is_baseline: bool) -> Result<(), WasmCheckError> {
    let cfg_path = config_path(&args.config);
    let exists = std::path::Path::new(&cfg_path).exists();

    let mut config = if exists {
        Config::load(&cfg_path)?
    } else if is_baseline {
        return Err(WasmCheckError::ConfigNotFound(cfg_path));
    } else {
        Config::default()
    };

    let files = resolve_files(&args.file, None)?;

    for file in &files {
        let report = SizeReport::measure(file)?;
        config.baseline.insert(file.clone(), report);
        if !config.files.contains(file) {
            config.files.push(file.clone());
        }
    }

    if !is_baseline && config.budget.is_none() {
        config.budget = Some(Budget {
            raw: Some(format!(
                "{:.0} KB",
                max_of(&config.baseline) as f64 / 1024.0
            )),
            gzip: None,
            brotli: None,
        });
    }

    config.save(&cfg_path)?;
    println!(
        "{} wrote {}",
        if is_baseline {
            "baseline updated:".green()
        } else {
            "done:".green()
        },
        cfg_path
    );
    Ok(())
}

fn max_of(baseline: &std::collections::BTreeMap<String, SizeReport>) -> u64 {
    baseline.values().map(|r| r.raw).max().unwrap_or(0)
}

fn print_table(
    report: &SizeReport,
    baseline: Option<&SizeReport>,
    budget: Option<&Budget>,
) -> Result<(), WasmCheckError> {
    println!();
    println!("  {} {}", "file:".dimmed(), report.file);

    let limits = budget.map(Budget::limits_bytes).transpose()?;

    for (i, metric) in Metric::ALL.iter().enumerate() {
        let value = metric.value(report);
        let mut line = format!(
            "  {:<10} {:>12}",
            metric.label().dimmed(),
            format_size(value)
        );

        if let Some(base) = baseline {
            let delta = delta_str(value, metric.value(base));
            let colored = if value > metric.value(base) {
                delta.red().to_string()
            } else if value < metric.value(base) {
                delta.green().to_string()
            } else {
                delta.dimmed().to_string()
            };
            line.push_str(&format!("  {colored}"));
        }

        if let Some(limits) = &limits
            && let Some(limit) = limits[i]
            && value > limit
        {
            line.push_str(&format!("  {}", "over budget".red().bold()));
        }
        println!("{line}");
    }
    println!();
    Ok(())
}

fn print_top(path: &str, n: usize) -> Result<(), WasmCheckError> {
    println!("  {} (best-effort):", "top N by function size".dimmed());
    let ranked = top_functions(path, n)?;
    if ranked.is_empty() {
        println!("    (no functions found)");
        return Ok(());
    }
    for (size, name) in ranked {
        let truncated: String = if name.len() > 60 {
            name.chars().take(57).collect::<String>() + "..."
        } else {
            name
        };
        println!("  {:<70} {:>12}", truncated.dimmed(), format_size(size));
    }
    println!();
    Ok(())
}
