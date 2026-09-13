//! The committed baseline, stored apart from the config.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::WasmCheckError;
use crate::size::SizeReport;

/// The recorded sizes of every checked file, keyed by the `files` entry that
/// selected it.
///
/// It lives in its own file — `.wasmcheck.baseline.json` next to the config by
/// default — so that a rebuild only ever rewrites that file. The config, the
/// part humans edit, stays untouched and therefore conflict-free in pull
/// requests.
///
/// The file is a plain JSON object, so it can also be produced by a CI job and
/// downloaded as an artifact:
///
/// ```json
/// {
///   "dist/assets/*_bg-*.wasm": {
///     "file": "dist/assets/app_bg-abc123.wasm",
///     "raw": 675212,
///     "gzip": 240928,
///     "brotli": 205110
///   }
/// }
/// ```
///
/// # Examples
///
/// ```
/// use wasmcheck::{Baseline, SizeReport};
///
/// let mut baseline = Baseline::new();
/// baseline.set("dist/*.wasm", SizeReport::new("dist/app.wasm", 1_000, 500, 400));
///
/// assert_eq!(baseline.get("dist/*.wasm").unwrap().raw(), 1_000);
/// assert_eq!(baseline.len(), 1);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Baseline {
    entries: BTreeMap<String, SizeReport>,
}

impl Baseline {
    /// An empty baseline.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Builds a baseline from already-keyed entries, typically to adopt a
    /// legacy inline baseline.
    #[must_use]
    pub const fn from_entries(entries: BTreeMap<String, SizeReport>) -> Self {
        Self { entries }
    }

    /// The number of recorded files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when nothing has been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, keyed by `files` entry.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &SizeReport)> {
        self.entries
            .iter()
            .map(|(key, report)| (key.as_str(), report))
    }

    /// The baseline recorded for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&SizeReport> {
        self.entries.get(key)
    }

    /// Records `report` under `key`.
    pub fn set(&mut self, key: impl Into<String>, report: SizeReport) {
        self.entries.insert(key.into(), report);
    }

    /// Forgets `key`.
    pub fn remove(&mut self, key: &str) -> Option<SizeReport> {
        self.entries.remove(key)
    }

    /// Drops entries that were produced by `pattern` under a previous filename
    /// (typically a stale `_bg-<hash>.wasm`) but are not in `keep`.
    ///
    /// Entries keyed by another glob or another path are left alone. This only
    /// has to clean up baselines written by earlier versions that keyed by the
    /// exact path.
    pub fn prune(&mut self, pattern: &str, keep: &[String]) {
        if !crate::config::is_glob(pattern) {
            return;
        }
        let Ok(glob) = glob::Pattern::new(pattern) else {
            return;
        };
        self.entries.retain(|key, _| {
            keep.iter().any(|kept| kept == key) || key == pattern || !glob.matches(key)
        });
    }

    /// Reads a baseline file.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::BaselineNotFound`] when the file does not
    /// exist, [`WasmCheckError::ConfigParse`] when it is not valid JSON, and
    /// [`WasmCheckError::Io`] for any other read failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wasmcheck::Baseline;
    ///
    /// let baseline = Baseline::load("dist/.wasmcheck.baseline.json")?;
    /// println!("{} files recorded", baseline.len());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn load(path: impl AsRef<Path>) -> Result<Self, WasmCheckError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                WasmCheckError::BaselineNotFound(path.to_path_buf())
            } else {
                WasmCheckError::Io(e)
            }
        })?;
        serde_json::from_str(&text).map_err(|source| WasmCheckError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Writes the baseline as pretty JSON with a trailing newline, creating
    /// parent directories as needed.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::Serialize`] when the baseline cannot be
    /// serialized, or [`WasmCheckError::Io`] when it cannot be written.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wasmcheck::Baseline;
    ///
    /// Baseline::new().save("target/.wasmcheck.baseline.json")?;
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), WasmCheckError> {
        let path = path.as_ref();
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, json).map_err(WasmCheckError::Io)
    }
}

impl From<BTreeMap<String, SizeReport>> for Baseline {
    fn from(entries: BTreeMap<String, SizeReport>) -> Self {
        Self::from_entries(entries)
    }
}
