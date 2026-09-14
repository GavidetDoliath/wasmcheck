//! The error type shared by every wasmcheck operation.

use std::path::PathBuf;

/// Everything that can go wrong while reading a config, measuring a bundle or
/// parsing a wasm module.
///
/// The enum is `#[non_exhaustive]`: match on the variants you care about and
/// keep a wildcard arm. When all you need is the CI outcome, prefer
/// [`WasmCheckError::is_budget_exceeded`].
///
/// # Examples
///
/// ```
/// use wasmcheck::WasmCheckError;
///
/// let err = "nope".parse::<wasmcheck::Size>().unwrap_err();
/// assert!(matches!(err, WasmCheckError::InvalidSize(_)));
/// assert!(!err.is_budget_exceeded());
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WasmCheckError {
    /// No `.wasm` file was found in the directory that was searched.
    ///
    /// The message carries the three ways out, so a first run on a real
    /// project — where the artifact lives under `dist/` or `target/` — points
    /// at the fix instead of dead-ending.
    #[error(
        "no .wasm file found in `{}`\n  tip: pass --file with the artifact path, e.g. target/wasm32-unknown-unknown/release/app.wasm\n  tip: --file accepts globs, e.g. --file \"dist/*_bg-*.wasm\"\n  tip: or list paths and globs under `files` in .wasmcheck.json",
        .dir.display()
    )]
    NoWasmFound {
        /// The directory that was searched.
        dir: PathBuf,
    },

    /// More than one candidate `.wasm` file was found and no file was chosen.
    #[error("multiple .wasm files found: {found:?}; pass --file to select one")]
    MultipleWasmFound {
        /// The candidates that were found.
        found: Vec<String>,
    },

    /// A path that should exist does not.
    #[error("file not found: {}", .0.display())]
    FileNotFound(PathBuf),

    /// The requested config file does not exist.
    #[error("config not found: {}", .0.display())]
    ConfigNotFound(PathBuf),

    /// A baseline file was requested explicitly but does not exist.
    #[error("baseline not found: {}", .0.display())]
    BaselineNotFound(PathBuf),

    /// An operating system error while reading or writing a file.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The config file exists but is not valid JSON for this version.
    #[error("failed to parse {}: {source}", .path.display())]
    ConfigParse {
        /// The config file that failed to parse.
        path: PathBuf,
        /// The underlying JSON error.
        source: serde_json::Error,
    },

    /// A budget string could not be read as a byte size.
    #[error(
        "invalid size `{0}`; expected a non-negative number like \"280 KB\" (percentages such as \"5%\" are only allowed in `max_delta`)"
    )]
    InvalidSize(String),

    /// A gzip compression level outside `0..=9`.
    #[error("invalid gzip level {0}; expected 0-9")]
    InvalidGzipLevel(i64),

    /// A brotli quality outside `0..=11`.
    #[error("invalid brotli quality {0}; expected 0-11")]
    InvalidBrotliQuality(i64),

    /// A threshold string could not be read as a percentage.
    #[error(
        "invalid percentage `{0}`; expected a non-negative number with at most two decimals, like \"5%\""
    )]
    InvalidPercent(String),

    /// A metric name could not be recognized.
    #[error("unknown metric `{0}`; expected one of: raw, gzip, brotli")]
    InvalidMetric(String),

    /// The run finished but at least one file failed a size gate: it is over
    /// its budget, or it grew past the configured `max_delta`.
    ///
    /// The payload is the message shown to the user; the per-file reasons are
    /// printed before this error is returned.
    #[error("{0}")]
    BudgetExceeded(String),

    /// The config could not be serialized back to JSON.
    #[error("failed to serialize config: {0}")]
    Serialize(#[from] serde_json::Error),

    /// A `.wasm` file could not be parsed by `wasmparser`.
    #[error("wasm parse error: {0}")]
    WasmParse(String),
}

impl WasmCheckError {
    /// `true` when a size gate failed — a file is over its budget or grew past
    /// `max_delta` — rather than the run failing to start at all.
    ///
    /// The `wasmcheck` binary exits with code `1` for both cases, but callers
    /// embedding the library usually want to tell them apart.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::WasmCheckError;
    ///
    /// let over = WasmCheckError::BudgetExceeded("app.wasm".into());
    /// assert!(over.is_budget_exceeded());
    /// ```
    #[must_use]
    pub fn is_budget_exceeded(&self) -> bool {
        matches!(self, WasmCheckError::BudgetExceeded(_))
    }
}
