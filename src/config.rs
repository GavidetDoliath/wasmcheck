//! The committed `.wasmcheck.json` config.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compression::Compression;
use crate::error::WasmCheckError;
use crate::limits::{Budget, MaxDelta};
use crate::size::SizeReport;

/// The contents of a `.wasmcheck.json` file.
///
/// Entries in [`Config::files`] are resolved **relative to the directory
/// containing the config file**, not relative to the process working directory.
/// That way `wasmcheck check --config dogfood-app/.wasmcheck.json` works from
/// the repository root. The same applies to [`Config::baseline_path`].
///
/// The recorded sizes live in a separate [`Baseline`](crate::Baseline) file, so
/// this one only changes when a human changes it.
///
/// # Examples
///
/// ```no_run
/// use wasmcheck::Config;
///
/// let config = Config::load("dogfood-app/.wasmcheck.json")?;
/// for entry in config.files() {
///     println!("{entry}");
/// }
/// println!("baseline: {}", config.baseline_path("dogfood-app").display());
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// File paths and globs to check.
    files: Vec<String>,
    /// Optional absolute caps, one per metric.
    #[serde(skip_serializing_if = "Option::is_none")]
    budget: Option<Budget>,
    /// Optional tolerated growth over the baseline, one per metric.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_delta: Option<MaxDelta>,
    /// Gzip level and brotli quality used to measure. Defaults to
    /// [`Compression::cdn`], and is omitted when left at that default.
    #[serde(skip_serializing_if = "Compression::is_default")]
    compression: Compression,
    /// Where the baseline lives, relative to the config file's directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_file: Option<String>,
    /// Baseline sizes kept inline. Deprecated: read for compatibility and
    /// moved to the baseline file by `init` and `baseline`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    baseline: BTreeMap<String, SizeReport>,
    /// Fail instead of tolerating a checked file that has no baseline entry.
    strict: bool,
}

impl Config {
    /// An empty config: no files, no budget, no baseline.
    ///
    /// An empty `files` list means "auto-detect a single `.wasm` file next to
    /// the config".
    #[must_use]
    pub const fn new() -> Self {
        Self {
            files: Vec::new(),
            budget: None,
            max_delta: None,
            compression: Compression::cdn(),
            baseline_file: None,
            baseline: BTreeMap::new(),
            strict: false,
        }
    }

    /// The configured file paths and globs, as written in the config.
    #[must_use]
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Adds `file` to the list unless it is already there.
    pub fn add_file(&mut self, file: impl Into<String>) {
        let file = file.into();
        if !self.files.contains(&file) {
            self.files.push(file);
        }
    }

    /// The configured budget, if any.
    #[must_use]
    pub fn budget(&self) -> Option<&Budget> {
        self.budget.as_ref()
    }

    /// Replaces the budget.
    pub fn set_budget(&mut self, budget: Option<Budget>) {
        self.budget = budget;
    }

    /// The maximum tolerated growth over the baseline, if configured.
    ///
    /// Files without a baseline entry cannot be evaluated against it.
    #[must_use]
    pub fn max_delta(&self) -> Option<&MaxDelta> {
        self.max_delta.as_ref()
    }

    /// Replaces the maximum tolerated growth over the baseline.
    pub fn set_max_delta(&mut self, max_delta: Option<MaxDelta>) {
        self.max_delta = max_delta;
    }

    /// The gzip level and brotli quality to measure with.
    ///
    /// Defaults to [`Compression::cdn`] — what a server or CDN does on the fly
    /// — rather than the best compression achievable, so a budget is not
    /// compared against a size that never reaches the wire.
    #[must_use]
    pub const fn compression(&self) -> Compression {
        self.compression
    }

    /// Replaces the compression setting.
    pub fn set_compression(&mut self, compression: Compression) {
        self.compression = compression;
    }

    /// The configured baseline file name, if the config names one.
    #[must_use]
    pub fn baseline_file(&self) -> Option<&str> {
        self.baseline_file.as_deref()
    }

    /// Points the config at a different baseline file, relative to the config
    /// file's directory.
    pub fn set_baseline_file(&mut self, file: Option<String>) {
        self.baseline_file = file;
    }

    /// Where the baseline file lives, given the directory holding this config.
    ///
    /// Defaults to [`DEFAULT_BASELINE_FILE`](crate::DEFAULT_BASELINE_FILE) next
    /// to the config.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::{Config, DEFAULT_BASELINE_FILE};
    ///
    /// let mut config = Config::new();
    /// assert!(config.baseline_path("dist").ends_with(DEFAULT_BASELINE_FILE));
    ///
    /// config.set_baseline_file(Some("ci/baseline.json".into()));
    /// assert!(config.baseline_path("dist").ends_with("ci/baseline.json"));
    /// ```
    #[must_use]
    pub fn baseline_path(&self, config_dir: impl AsRef<Path>) -> PathBuf {
        let dir = config_dir.as_ref();
        let dir = if dir == Path::new(".") || dir.as_os_str().is_empty() {
            Path::new("")
        } else {
            dir
        };
        match &self.baseline_file {
            Some(file) => dir.join(file),
            None => dir.join(crate::DEFAULT_BASELINE_FILE),
        }
    }

    /// The deprecated baseline still stored inline in the config.
    ///
    /// New configs do not have one: sizes live in the
    /// [`Baseline`](crate::Baseline) file. When no baseline file exists yet, a
    /// config carrying this map is the one authoritative source, so callers
    /// should adopt it before writing the baseline file.
    #[must_use]
    pub fn inline_baseline(&self) -> &BTreeMap<String, SizeReport> {
        &self.baseline
    }

    /// Takes the deprecated inline baseline out of the config.
    #[must_use]
    pub fn take_inline_baseline(&mut self) -> BTreeMap<String, SizeReport> {
        std::mem::take(&mut self.baseline)
    }

    /// Drops the deprecated inline baseline.
    ///
    /// `init` and `baseline` call this after writing the baseline file, so the
    /// next save stops carrying hundreds of lines of machine-generated data.
    pub fn clear_inline_baseline(&mut self) {
        self.baseline.clear();
    }

    /// Whether a checked file without a baseline entry fails the run.
    ///
    /// Off by default: a fresh checkout with no baseline still reports sizes,
    /// it just cannot gate on growth.
    #[must_use]
    pub const fn strict(&self) -> bool {
        self.strict
    }

    /// Enables or disables strict mode.
    pub fn set_strict(&mut self, strict: bool) {
        self.strict = strict;
    }

    /// Reads and parses a config file.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::ConfigNotFound`] when the file does not exist,
    /// [`WasmCheckError::ConfigParse`] when it is not valid config JSON, and
    /// [`WasmCheckError::Io`] for any other read failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wasmcheck::Config;
    ///
    /// let config = Config::load(".wasmcheck.json")?;
    /// assert!(config.budget().is_some());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn load(path: impl AsRef<Path>) -> Result<Self, WasmCheckError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                WasmCheckError::ConfigNotFound(path.to_path_buf())
            } else {
                WasmCheckError::Io(e)
            }
        })?;
        serde_json::from_str(&text).map_err(|source| WasmCheckError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Writes the config as pretty JSON with a trailing newline.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::Serialize`] if the config cannot be
    /// serialized, or [`WasmCheckError::Io`] if the file cannot be written.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wasmcheck::Config;
    ///
    /// Config::new().save(".wasmcheck.json")?;
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), WasmCheckError> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        std::fs::write(path, json).map_err(WasmCheckError::Io)
    }
}

/// `true` when `entry` contains glob metacharacters.
#[must_use]
pub(crate) fn is_glob(entry: &str) -> bool {
    entry.contains(['*', '?', '[', ']'])
}
