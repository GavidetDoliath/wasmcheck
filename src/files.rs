//! Locating the `.wasm` files to check.

use std::path::{Path, PathBuf};

use crate::config::{Config, is_glob};
use crate::error::WasmCheckError;

/// A `.wasm` file selected for checking.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use wasmcheck::{Config, resolve_files};
///
/// // With no config and one .wasm in the directory, it is auto-detected.
/// let files = resolve_files(Some("fixtures/full.wasm"), None, Path::new("."))?;
/// assert_eq!(files[0].key(), "fixtures/full.wasm");
/// assert!(files[0].path().ends_with("full.wasm"));
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFile {
    key: String,
    path: PathBuf,
}

impl ResolvedFile {
    /// The config entry that selected this file: a glob, a path, or the path
    /// passed on the command line.
    ///
    /// Baselines are keyed by this value, which is what keeps deltas working
    /// when a build emits `app_bg-<newhash>.wasm` behind a stable glob.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The path to open and measure.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The `.wasm` files directly inside `dir`, sorted by path.
///
/// The search is not recursive: point the config `files` list at a glob such as
/// `dist/assets/*.wasm` to reach into subdirectories.
///
/// # Errors
///
/// Returns [`WasmCheckError::Io`] when `dir` cannot be read.
///
/// # Examples
///
/// ```
/// use wasmcheck::find_wasm_files;
///
/// let found = find_wasm_files("fixtures")?;
/// assert_eq!(found.len(), 2);
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
pub fn find_wasm_files(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>, WasmCheckError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir.as_ref())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
        .collect();
    entries.sort();
    Ok(entries)
}

/// Decides which `.wasm` files a run should measure.
///
/// Resolution order:
///
/// 1. `cli_file`, when given on the command line (relative to the current
///    working directory);
/// 2. the `files` list of `config`, resolved **relative to `config_dir`** —
///    each entry is a path or a glob, and every file matched by a glob shares
///    that glob as its [`ResolvedFile::key`];
/// 3. otherwise, a single `.wasm` file auto-detected in `config_dir`.
///
/// # Errors
///
/// Returns [`WasmCheckError::FileNotFound`] for a missing explicit path,
/// [`WasmCheckError::NoWasmFound`] when nothing matched, and
/// [`WasmCheckError::MultipleWasmFound`] when auto-detection is ambiguous.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use wasmcheck::{Config, resolve_files};
///
/// let config = Config::load("dist/.wasmcheck.json")?;
/// let files = resolve_files(None, Some(&config), Path::new("dist"))?;
/// for file in files {
///     println!("{} -> {}", file.key(), file.path().display());
/// }
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
pub fn resolve_files(
    cli_file: Option<&str>,
    config: Option<&Config>,
    config_dir: &Path,
) -> Result<Vec<ResolvedFile>, WasmCheckError> {
    let base = normalized_dir(config_dir);

    if let Some(file) = cli_file {
        let path = clean(Path::new(file));
        if !path.is_file() {
            return Err(WasmCheckError::FileNotFound(path));
        }
        return Ok(vec![ResolvedFile {
            key: key_for(&path, &base),
            path,
        }]);
    }

    if let Some(config) = config.filter(|c| !c.files().is_empty()) {
        let mut resolved = Vec::new();
        for entry in config.files() {
            if is_glob(entry) {
                let pattern = glob_pattern(&base, entry);
                let matches = glob::glob(&pattern)
                    .map_err(|e| WasmCheckError::Io(std::io::Error::other(e.to_string())))?;
                for path in matches.filter_map(Result::ok).filter(|p| p.is_file()) {
                    resolved.push(ResolvedFile {
                        key: entry.clone(),
                        path: clean(&path),
                    });
                }
            } else {
                let path = clean(&base.join(entry));
                if !path.is_file() {
                    return Err(WasmCheckError::FileNotFound(path));
                }
                resolved.push(ResolvedFile {
                    key: entry.clone(),
                    path,
                });
            }
        }

        resolved.sort_by(|a, b| a.path.cmp(&b.path));
        resolved.dedup();
        if resolved.is_empty() {
            return Err(WasmCheckError::NoWasmFound {
                dir: display_dir(&base),
            });
        }
        return Ok(resolved);
    }

    let dir = if base.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        base.clone()
    };
    let found = find_wasm_files(&dir)?;
    match found.as_slice() {
        [] => Err(WasmCheckError::NoWasmFound {
            dir: display_dir(&base),
        }),
        [only] => Ok(vec![ResolvedFile {
            key: file_name(only),
            path: clean(only),
        }]),
        many => Err(WasmCheckError::MultipleWasmFound {
            found: many.iter().map(|p| p.display().to_string()).collect(),
        }),
    }
}

/// Treats `""` and `"."` the same so resolved paths stay free of a `./` prefix.
fn normalized_dir(dir: &Path) -> PathBuf {
    if dir.as_os_str().is_empty() || dir == Path::new(".") {
        PathBuf::new()
    } else {
        dir.to_path_buf()
    }
}

fn display_dir(base: &Path) -> PathBuf {
    if base.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        base.to_path_buf()
    }
}

fn glob_pattern(base: &Path, entry: &str) -> String {
    if base.as_os_str().is_empty() {
        entry.to_string()
    } else {
        format!("{}/{}", base.to_string_lossy().replace('\\', "/"), entry)
    }
}

fn key_for(path: &Path, base: &Path) -> String {
    if base.as_os_str().is_empty() {
        return path.display().to_string();
    }
    match path.strip_prefix(base) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
        _ => path.display().to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn clean(path: &Path) -> PathBuf {
    path.strip_prefix(".")
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}
