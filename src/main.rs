use std::path::{Path, PathBuf};
use std::process;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand, ValueEnum};
use owo_colors::OwoColorize;
use serde::Serialize;

use wasmcheck::{
    Baseline, Budget, CONFIG_FILE, Compression, Config, DEFAULT_BASELINE_FILE, MaxDelta, Metric,
    ResolvedFile, Size, SizeDelta, SizeReport, WasmCheckError, delta_str, format_size,
    resolve_files, top_functions,
};

/// Run-level and per-file verdicts used by the JSON output.
const PASS: &str = "pass";
const FAIL: &str = "fail";
const UNCHECKED: &str = "unchecked";

#[derive(Parser)]
#[command(name = "wasmcheck", version, about = "WASM bundle size budget checker")]
struct Cli {
    /// Path to a specific .wasm file
    #[arg(short, long)]
    file: Option<String>,

    /// Size budget (e.g. "280 KB", "2.5 MB") — applies to raw size
    #[arg(short, long)]
    budget: Option<String>,

    /// Fail when a file grows by more than SIZE (e.g. "50 KB", "5%") over its
    /// baseline, even while under budget — applies to raw size
    #[arg(long, value_name = "SIZE")]
    max_delta: Option<String>,

    /// Fail when a checked file has no baseline entry
    #[arg(long)]
    strict: bool,

    /// Baseline file to read (default: the config's `baseline_file`, else
    /// `.wasmcheck.baseline.json` next to the config)
    #[arg(long, value_name = "PATH")]
    baseline_file: Option<String>,

    /// Gzip level to measure with, 0-9 (default: 6, what a CDN does on the fly)
    #[arg(long, value_name = "0-9")]
    gzip_level: Option<u32>,

    /// Brotli quality to measure with, 0-11 (default: 5)
    #[arg(long, value_name = "0-11")]
    brotli_quality: Option<i32>,

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

    /// Fail when a file grows by more than SIZE (e.g. "50 KB", "5%") over its
    /// baseline, even while under budget — applies to raw size
    #[arg(long, value_name = "SIZE")]
    max_delta: Option<String>,

    /// Fail when a checked file has no baseline entry
    #[arg(long)]
    strict: bool,

    /// Baseline file to read (default: the config's `baseline_file`, else
    /// `.wasmcheck.baseline.json` next to the config)
    #[arg(long, value_name = "PATH")]
    baseline_file: Option<String>,

    /// Path to config file (default: ./.wasmcheck.json if present)
    #[arg(short, long)]
    config: Option<String>,

    /// Gzip level to measure with, 0-9 (default: 6, what a CDN does on the fly)
    #[arg(long, value_name = "0-9")]
    gzip_level: Option<u32>,

    /// Brotli quality to measure with, 0-11 (default: 5)
    #[arg(long, value_name = "0-11")]
    brotli_quality: Option<i32>,

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

    /// Baseline file to write (default: the config's `baseline_file`, else
    /// `.wasmcheck.baseline.json` next to the config)
    #[arg(long, value_name = "PATH")]
    baseline_file: Option<String>,
}

#[derive(Clone, ValueEnum)]
enum Format {
    Table,
    Json,
    Github,
}

/// `{"status": …, "files": [...]}` — the whole run in one JSON document, so the
/// output stays parseable whatever the number of files.
#[derive(Serialize)]
struct JsonRun<'a> {
    status: &'static str,
    /// The compression the numbers were measured with, so a CI report can be
    /// reproduced.
    compression: Compression,
    files: Vec<JsonFile<'a>>,
}

#[derive(Serialize)]
struct JsonFile<'a> {
    file: &'a str,
    raw: u64,
    gzip: u64,
    brotli: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline: Option<&'a SizeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<SizeDelta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget: Option<Budget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_delta: Option<MaxDelta>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exceeded: Vec<Metric>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    delta_exceeded: Vec<Metric>,
    #[serde(skip_serializing_if = "is_false")]
    baseline_missing: bool,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    top: Option<Vec<JsonFunction>>,
}

#[derive(Serialize)]
struct JsonFunction {
    size: u64,
    name: String,
}

/// One measured file, with everything the output formats need.
struct Measured<'a> {
    file: &'a ResolvedFile,
    report: SizeReport,
    baseline: Option<&'a SizeReport>,
    exceeded: Vec<Metric>,
    delta_exceeded: Vec<Metric>,
    /// `true` when strict mode is on and this file has no baseline to compare.
    missing_baseline: bool,
}

impl Measured<'_> {
    fn failed(&self) -> bool {
        !self.exceeded.is_empty() || !self.delta_exceeded.is_empty() || self.missing_baseline
    }
}

/// The gates in force for a run.
struct Gates<'a> {
    budget: Option<&'a Budget>,
    max_delta: Option<&'a MaxDelta>,
    strict: bool,
}

impl Gates<'_> {
    /// `true` when at least one gate is configured, i.e. the run can fail.
    fn enabled(&self) -> bool {
        self.budget.is_some() || self.max_delta.is_some() || self.strict
    }
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
            max_delta: cli.max_delta,
            strict: cli.strict,
            baseline_file: cli.baseline_file,
            gzip_level: cli.gzip_level,
            brotli_quality: cli.brotli_quality,
            config: None,
            format: cli.format,
            top: cli.top,
        }),
        Some(Commands::Check(args)) => run_check(args),
        Some(Commands::Init(args)) => run_init(&args),
        Some(Commands::Baseline(args)) => run_baseline(&args),
    }
}

fn config_path(explicit: &Option<String>) -> PathBuf {
    explicit
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(CONFIG_FILE))
}

/// The directory a config's `files` entries are resolved against.
fn config_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Loads the config when it exists, together with its base directory.
fn load_config(explicit: &Option<String>) -> Result<(Option<Config>, PathBuf), WasmCheckError> {
    let path = config_path(explicit);
    if path.exists() {
        Ok((Some(Config::load(&path)?), config_dir(&path)))
    } else if explicit.is_some() {
        Err(WasmCheckError::ConfigNotFound(path))
    } else {
        Ok((None, PathBuf::from(".")))
    }
}

/// Where the baseline lives: `--baseline-file`, else the config's
/// `baseline_file`, else [`DEFAULT_BASELINE_FILE`] next to the config.
fn baseline_path(config: Option<&Config>, config_dir: &Path, explicit: &Option<String>) -> PathBuf {
    if let Some(path) = explicit {
        return PathBuf::from(path);
    }
    if let Some(config) = config {
        return config.baseline_path(config_dir);
    }
    let dir = if config_dir == Path::new(".") {
        Path::new("")
    } else {
        config_dir
    };
    dir.join(DEFAULT_BASELINE_FILE)
}

/// `true` when the baseline location was named by the user, in which case a
/// missing file is an error rather than "nothing recorded yet".
fn baseline_is_explicit(config: Option<&Config>, explicit: &Option<String>) -> bool {
    explicit.is_some() || config.is_some_and(|config| config.baseline_file().is_some())
}

/// Loads the baseline file, adopting a legacy inline baseline when no file
/// exists yet (the config used to carry the sizes itself).
fn load_baseline(
    config: Option<&Config>,
    path: &Path,
    missing_is_error: bool,
) -> Result<Baseline, WasmCheckError> {
    if path.exists() {
        return Baseline::load(path);
    }
    if missing_is_error {
        return Err(WasmCheckError::BaselineNotFound(path.to_path_buf()));
    }
    match config {
        Some(config) if !config.inline_baseline().is_empty() => {
            Ok(Baseline::from_entries(config.inline_baseline().clone()))
        }
        _ => Ok(Baseline::new()),
    }
}

/// The measurement setting for a run: the config's, with any CLI flag
/// overriding the matching field.
fn effective_compression(
    gzip_level: Option<u32>,
    brotli_quality: Option<i32>,
    config: Option<&Config>,
) -> Result<Compression, WasmCheckError> {
    let mut compression = match config {
        Some(config) => config.compression(),
        None => Compression::default(),
    };
    if let Some(level) = gzip_level {
        compression = compression.with_gzip_level(level)?;
    }
    if let Some(quality) = brotli_quality {
        compression = compression.with_brotli_quality(quality)?;
    }
    Ok(compression)
}

/// A CLI flag overrides only the raw limit of the configured ones.
fn effective_limits<T>(
    cli_limit: Option<&str>,
    configured: Option<&wasmcheck::Limits<T>>,
) -> Result<Option<wasmcheck::Limits<T>>, WasmCheckError>
where
    T: Clone + FromStr<Err = WasmCheckError>,
{
    match (cli_limit, configured) {
        (Some(raw), Some(configured)) => {
            let mut limits = configured.clone();
            limits.set_limit(Metric::Raw, Some(raw.parse()?));
            Ok(Some(limits))
        }
        (Some(raw), None) => {
            let mut limits = wasmcheck::Limits::new();
            limits.set_limit(Metric::Raw, Some(raw.parse()?));
            Ok(Some(limits))
        }
        (None, Some(configured)) => Ok(Some(configured.clone())),
        (None, None) => Ok(None),
    }
}

fn run_check(args: CheckArgs) -> Result<(), WasmCheckError> {
    let (config, dir) = load_config(&args.config)?;
    let budget = effective_limits(
        args.budget.as_deref(),
        config.as_ref().and_then(Config::budget),
    )?;
    let max_delta = effective_limits(
        args.max_delta.as_deref(),
        config.as_ref().and_then(Config::max_delta),
    )?;
    let gates = Gates {
        budget: budget.as_ref(),
        max_delta: max_delta.as_ref(),
        strict: args.strict || config.as_ref().is_some_and(Config::strict),
    };

    let compression = effective_compression(args.gzip_level, args.brotli_quality, config.as_ref())?;
    let baseline_file = baseline_path(config.as_ref(), &dir, &args.baseline_file);
    let baseline = load_baseline(
        config.as_ref(),
        &baseline_file,
        baseline_is_explicit(config.as_ref(), &args.baseline_file),
    )?;

    let files = resolve_files(args.file.as_deref(), config.as_ref(), &dir)?;

    let mut measured = Vec::with_capacity(files.len());
    for file in &files {
        let report = SizeReport::measure_with(file.path(), compression)?;
        let baseline = baseline.get(file.key());
        let exceeded = gates
            .budget
            .map(|budget| budget.exceeded_metrics(&report))
            .unwrap_or_default();
        // Without a recorded baseline there is nothing to compare against, so
        // `max_delta` cannot fail the run; `--strict` is what gates that case.
        let delta_exceeded = match (gates.max_delta, baseline) {
            (Some(max_delta), Some(baseline)) => max_delta.exceeded_delta(&report, baseline),
            _ => Vec::new(),
        };
        measured.push(Measured {
            file,
            report,
            baseline,
            exceeded,
            delta_exceeded,
            missing_baseline: gates.strict && baseline.is_none(),
        });
    }

    match args.format {
        Format::Table => print_table_run(&args, &measured, &gates)?,
        Format::Json => print_json_run(&args, &measured, &gates, compression)?,
        Format::Github => print_github_run(&measured, &gates)?,
    }

    if measured.iter().any(Measured::failed) {
        return Err(WasmCheckError::BudgetExceeded(
            "one or more files failed their size gate".to_string(),
        ));
    }
    Ok(())
}

fn print_table_run(
    args: &CheckArgs,
    measured: &[Measured<'_>],
    gates: &Gates<'_>,
) -> Result<(), WasmCheckError> {
    for entry in measured {
        print_table(entry, gates);
        if let Some(top) = args.top {
            print_top(entry.file.path(), top)?;
        }

        if !entry.missing_baseline && gates.max_delta.is_some() && entry.baseline.is_none() {
            eprintln!(
                "  {}",
                "no baseline recorded for this file: max delta not evaluated".dimmed()
            );
        }

        if gates.enabled() {
            if entry.failed() {
                eprintln!(
                    "\n{} {}: {}",
                    "FAIL".red().bold(),
                    entry.report.file().cyan(),
                    failure_reasons(entry)
                );
            } else {
                eprintln!("\n{} {}", "PASS".green().bold(), entry.report.file().cyan());
            }
        }
    }
    Ok(())
}

/// Writes the whole run as one JSON document on stdout. Progress lines stay on
/// stderr so that stdout is always parseable, even when a file fails.
fn print_json_run(
    args: &CheckArgs,
    measured: &[Measured<'_>],
    gates: &Gates<'_>,
    compression: Compression,
) -> Result<(), WasmCheckError> {
    let mut files = Vec::with_capacity(measured.len());
    for entry in measured {
        let top = match args.top {
            Some(n) => Some(
                top_functions(entry.file.path(), n)?
                    .into_iter()
                    .map(|(size, name)| JsonFunction { size, name })
                    .collect(),
            ),
            None => None,
        };

        files.push(JsonFile {
            file: entry.report.file(),
            raw: entry.report.raw(),
            gzip: entry.report.gzip(),
            brotli: entry.report.brotli(),
            baseline: entry.baseline,
            delta: entry.baseline.map(|baseline| entry.report.delta(baseline)),
            budget: gates.budget.cloned(),
            max_delta: gates.max_delta.cloned(),
            exceeded: entry.exceeded.clone(),
            delta_exceeded: entry.delta_exceeded.clone(),
            baseline_missing: entry.missing_baseline,
            status: file_status(entry, gates),
            top,
        });
    }

    let status = if files.iter().any(|file| file.status == FAIL) {
        FAIL
    } else if gates.enabled() {
        PASS
    } else {
        UNCHECKED
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&JsonRun {
            status,
            compression,
            files,
        })?
    );
    Ok(())
}

/// Emits GitHub Actions workflow commands, so failures show up as annotations
/// on the pull request that introduced them. `--top` is not part of this
/// format: annotations are for problems, not for reports.
fn print_github_run(measured: &[Measured<'_>], gates: &Gates<'_>) -> Result<(), WasmCheckError> {
    for entry in measured {
        let file = escape_property(entry.report.file());

        if entry.missing_baseline {
            println!("::error file={file},title=wasmcheck::no baseline recorded for this file");
        }

        for metric in &entry.exceeded {
            let actual = format_size(entry.report.value(*metric));
            let limit = gates
                .budget
                .and_then(|budget| budget.limit(*metric))
                .map(|limit| format_size(limit.bytes()));
            let message = match limit {
                Some(limit) => format!("{metric} over budget: {actual} > {limit}"),
                None => format!("{metric} over budget: {actual}"),
            };
            println!(
                "::error file={file},title=wasmcheck::{}",
                escape_data(&message)
            );
        }

        for metric in &entry.delta_exceeded {
            let baseline = entry.baseline.map_or(0, |baseline| baseline.value(*metric));
            let growth = delta_str(entry.report.value(*metric), baseline);
            let allowance = gates
                .max_delta
                .and_then(|max_delta| max_delta.limit(*metric))
                .map_or_else(|| "?".to_string(), ToString::to_string);
            let message = format!("{metric} grew past max delta: {growth} > {allowance}");
            println!(
                "::error file={file},title=wasmcheck::{}",
                escape_data(&message)
            );
        }
    }
    Ok(())
}

fn file_status(entry: &Measured<'_>, gates: &Gates<'_>) -> &'static str {
    if !gates.enabled() {
        UNCHECKED
    } else if entry.failed() {
        FAIL
    } else {
        PASS
    }
}

/// A human summary of why a file failed, e.g.
/// `over budget: gzip; grows past max delta: raw`.
fn failure_reasons(entry: &Measured<'_>) -> String {
    let mut reasons = Vec::new();
    if entry.missing_baseline {
        reasons.push("no baseline recorded".to_string());
    }
    if !entry.exceeded.is_empty() {
        reasons.push(format!("over budget: {}", metric_list(&entry.exceeded)));
    }
    if !entry.delta_exceeded.is_empty() {
        reasons.push(format!(
            "grows past max delta: {}",
            metric_list(&entry.delta_exceeded)
        ));
    }
    reasons.join("; ")
}

fn metric_list(metrics: &[Metric]) -> String {
    metrics
        .iter()
        .map(Metric::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Escapes a workflow command message: `%`, carriage returns and newlines.
fn escape_data(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a value used in the `key=value` part of a workflow command, where
/// `:` and `,` would otherwise end the value or the property list.
fn escape_property(value: &str) -> String {
    escape_data(value).replace(':', "%3A").replace(',', "%2C")
}

/// Creates a config from the files currently on disk, or refreshes an existing
/// one. The seeded budget is rounded **up** so it never fails its own `check`.
fn run_init(args: &InitArgs) -> Result<(), WasmCheckError> {
    let path = config_path(&args.config);
    let mut config = if path.exists() {
        Config::load(&path)?
    } else {
        Config::new()
    };
    let dir = config_dir(&path);

    let baseline_file = baseline_path(Some(&config), &dir, &args.baseline_file);
    // A write command must be able to create the file it is pointed at.
    let mut baseline = load_baseline(Some(&config), &baseline_file, false)?;

    let files = resolve_files(args.file.as_deref(), Some(&config), &dir)?;
    let compression = config.compression();
    record_baseline(&mut config, &mut baseline, &files, compression)?;

    if config.budget().is_none() {
        let widest = baseline
            .iter()
            .map(|(_, report)| report.raw())
            .max()
            .unwrap_or(0);
        let mut budget = Budget::new();
        budget.set_limit(Metric::Raw, Some(Size::from_kib_ceil(widest)));
        config.set_budget(Some(budget));
    }

    baseline.save(&baseline_file)?;
    // Drop the deprecated inline copy now that the sizes have a home.
    config.clear_inline_baseline();
    config.save(&path)?;
    println!(
        "{} wrote {} and {}",
        "done:".green(),
        path.display(),
        baseline_file.display()
    );
    Ok(())
}

/// Re-measures the configured files and stores them as the new baseline.
fn run_baseline(args: &InitArgs) -> Result<(), WasmCheckError> {
    let path = config_path(&args.config);
    if !path.exists() {
        return Err(WasmCheckError::ConfigNotFound(path));
    }
    let mut config = Config::load(&path)?;
    let dir = config_dir(&path);

    let baseline_file = baseline_path(Some(&config), &dir, &args.baseline_file);
    let mut baseline = load_baseline(Some(&config), &baseline_file, false)?;

    let files = resolve_files(args.file.as_deref(), Some(&config), &dir)?;
    let compression = config.compression();
    record_baseline(&mut config, &mut baseline, &files, compression)?;

    baseline.save(&baseline_file)?;
    config.clear_inline_baseline();
    config.save(&path)?;
    println!(
        "{} wrote {}",
        "baseline updated:".green(),
        baseline_file.display()
    );
    Ok(())
}

/// Measures every resolved file and stores it as the baseline under the key
/// that selected it, dropping baselines left behind by an older content hash.
fn record_baseline(
    config: &mut Config,
    baseline: &mut Baseline,
    files: &[ResolvedFile],
    compression: Compression,
) -> Result<(), WasmCheckError> {
    let mut keys: Vec<String> = files.iter().map(|file| file.key().to_string()).collect();
    keys.sort();
    keys.dedup();

    for file in files {
        let report = SizeReport::measure_with(file.path(), compression)?;
        baseline.set(file.key(), report);
        config.add_file(file.key());
    }
    for key in &keys {
        baseline.prune(key, &keys);
    }
    Ok(())
}

fn print_table(entry: &Measured<'_>, gates: &Gates<'_>) {
    println!();
    println!("  {} {}", "file:".dimmed(), entry.report.file());

    for metric in Metric::ALL {
        let value = entry.report.value(metric);
        let padded = format!("{metric:<10}");
        let label = padded.dimmed().to_string();
        let mut line = format!("  {label} {:>12}", format_size(value));

        if let Some(baseline) = entry.baseline {
            let previous = baseline.value(metric);
            let delta = delta_str(value, previous);
            let colored = match value.cmp(&previous) {
                std::cmp::Ordering::Greater => delta.red().to_string(),
                std::cmp::Ordering::Less => delta.green().to_string(),
                std::cmp::Ordering::Equal => delta.dimmed().to_string(),
            };
            line.push_str("  ");
            line.push_str(&colored);
        }

        let over_budget = gates
            .budget
            .and_then(|budget| budget.limit(metric))
            .is_some_and(|limit| value > limit.bytes());
        if over_budget {
            line.push_str(&format!("  {}", "over budget".red().bold()));
        }

        if entry.delta_exceeded.contains(&metric) {
            line.push_str(&format!("  {}", "over max delta".red().bold()));
        }
        println!("{line}");
    }
    println!();
}

fn print_top(path: &Path, n: usize) -> Result<(), WasmCheckError> {
    println!("  {} (best-effort):", "top N by function size".dimmed());
    let ranked = top_functions(path, n)?;
    if ranked.is_empty() {
        println!("    (no functions found)");
        return Ok(());
    }
    for (size, name) in ranked {
        let truncated = if name.len() > 60 {
            name.chars().take(57).collect::<String>() + "..."
        } else {
            name
        };
        let padded = format!("{truncated:<70}").dimmed().to_string();
        println!("  {padded} {:>12}", format_size(size));
    }
    println!();
    Ok(())
}
