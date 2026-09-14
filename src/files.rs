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
/// When nothing sits directly in `dir`, the search falls back into the
/// conventional Rust→wasm output spots:
///
/// - one level down: `dir/dist/*.wasm` and `dir/target/*.wasm`;
/// - the cargo wasm layout: `dir/target/wasm32-unknown-unknown/{release,debug}/*.wasm`.
///
/// Deeper or hashed tooling layouts (dx's `dist/assets/app_bg-<hash>.wasm`,
/// trunk's `dist/pkg-*`) stay out of reach: auto-detection must stay
/// predictable, and the fallback must not wander into `target/**` noise.
/// Point the config `files` list at a glob such as `dist/assets/*.wasm`, or
/// pass `--file "target/**/*_bg-*.wasm"`, for those.
///
/// The paths of the returned files keep their fallback prefix, so a hit in
/// `dist` reads as `dist/app.wasm`.
///
/// # Errors
///
/// Returns [`WasmCheckError::Io`] when `dir` cannot be read. Unreadable
/// fallback subdirectories are skipped.
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

    if entries.is_empty() {
        for sub in FALLBACK_DIRS {
            let sub = dir.as_ref().join(sub);
            if let Ok(found) = find_wasm_files(&sub) {
                entries.extend(found);
            }
        }
        for sub in CARGO_WASM_DIRS {
            let sub = dir.as_ref().join(sub);
            if let Ok(found) = find_wasm_files(&sub) {
                entries.extend(found);
            }
        }
        entries.sort();
    }
    Ok(entries)
}

/// Conventional output directories the auto-detect fallback descends into,
/// one level below the searched directory.
const FALLBACK_DIRS: [&str; 2] = ["dist", "target"];

/// Exact cargo output directories for wasm targets. Only reached when the
/// searched directory itself holds no `.wasm` file, so this cannot be fooled
/// by an unrelated `target/**` layout.
const CARGO_WASM_DIRS: [&str; 2] = [
    "target/wasm32-unknown-unknown/release",
    "target/wasm32-unknown-unknown/debug",
];

/// Decides which `.wasm` files a run should measure.
///
/// Resolution order:
///
/// 1. `cli_file`, when given on the command line — a path **or a glob**, like
///    the config entries; every file matched by a glob shares that glob as its
///    [`ResolvedFile::key`], so `init --file "dist/*_bg-*.wasm"` records the
///    glob itself and a rebuild under a new content hash keeps its delta;
/// 2. the `files` list of `config`, resolved **relative to `config_dir`** —
///    each entry is a path or a glob, and every file matched by a glob shares
///    that glob as its [`ResolvedFile::key`];
/// 3. otherwise, a single `.wasm` file auto-detected in `config_dir`, falling
///    back into its `dist/` and `target/` subdirectories — one level deep, or
///    exactly at the cargo wasm layout
///    `target/wasm32-unknown-unknown/{release,debug}/`.
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
        if is_glob(file) {
            let pattern = glob_pattern(&base, file);
            let matches = glob::glob(&pattern)
                .map_err(|e| WasmCheckError::Io(std::io::Error::other(e.to_string())))?;
            let mut resolved = Vec::new();
            for path in matches.filter_map(Result::ok).filter(|p| p.is_file()) {
                resolved.push(ResolvedFile {
                    key: file.to_string(),
                    path: clean(&path),
                });
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
        [only] => {
            let path = clean(only);
            Ok(vec![ResolvedFile {
                // The path relative to the config dir, not the bare file
                // name: `init` records it in the config `files` list, and a
                // fallback hit in `dist/` or `target/` must resolve again on
                // the next run.
                key: key_for(&path, &base),
                path,
            }])
        }
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

fn clean(path: &Path) -> PathBuf {
    path.strip_prefix(".")
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    /// Creates `path` with the given contents, creating parent directories.
    fn write(path: &str, contents: &[u8]) {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn finds_wasm_directly_in_dir() {
        let dir = std::env::temp_dir().join("wasmcheck-direct");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create dir");
        write(&dir.join("app.wasm").display().to_string(), b"\0asm stub");

        let found = find_wasm_files(&dir).expect("find");
        assert_eq!(found, vec![dir.join("app.wasm")]);

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn falls_back_one_level_into_dist() {
        let dir = std::env::temp_dir().join("wasmcheck-fallback-dist");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("dist").join("app.wasm").display().to_string(),
            b"\0asm stub",
        );

        let found = find_wasm_files(&dir).expect("find");
        assert_eq!(found, vec![dir.join("dist").join("app.wasm")]);

        fs::remove_dir_all(&dir).expect("cleanup");
    }
    #[test]
    fn falls_back_one_level_into_target() {
        let dir = std::env::temp_dir().join("wasmcheck-fallback-target");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("target").join("app.wasm").display().to_string(),
            b"\0asm stub",
        );

        let found = find_wasm_files(&dir).expect("find");
        assert_eq!(found, vec![dir.join("target").join("app.wasm")]);

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn does_not_recurse_past_one_level() {
        let dir = std::env::temp_dir().join("wasmcheck-too-deep");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("build")
                .join("output")
                .join("app.wasm")
                .display()
                .to_string(),
            b"\0asm stub",
        );

        assert!(find_wasm_files(&dir).expect("find").is_empty());

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn direct_files_win_over_fallback() {
        let dir = std::env::temp_dir().join("wasmcheck-direct-wins");
        let _ = fs::remove_dir_all(&dir);
        write(&dir.join("top.wasm").display().to_string(), b"\0asm stub");
        write(
            &dir.join("dist").join("app.wasm").display().to_string(),
            b"\0asm stub",
        );

        let found = find_wasm_files(&dir).expect("find");
        assert_eq!(found, vec![dir.join("top.wasm")]);

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn fallback_key_is_relative_to_the_search_dir() {
        let dir = std::env::temp_dir().join("wasmcheck-fallback-key");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("dist").join("app.wasm").display().to_string(),
            b"\0asm stub",
        );

        let resolved = resolve_files(None, None, &dir).expect("resolve");
        assert_eq!(resolved.len(), 1);
        // `init` writes this key into the config `files` list; it must resolve
        // again relative to the config dir on the next run.
        assert_eq!(resolved[0].key(), "dist/app.wasm");

        fs::remove_dir_all(&dir).expect("cleanup");
    }
    #[test]
    fn finds_the_cargo_wasm_layout() {
        let dir = std::env::temp_dir().join("wasmcheck-cargo-wasm-layout");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("target")
                .join("wasm32-unknown-unknown")
                .join("release")
                .join("app.wasm")
                .display()
                .to_string(),
            b"\0asm stub",
        );

        let found = find_wasm_files(&dir).expect("find");
        assert_eq!(
            found,
            vec![
                dir.join("target")
                    .join("wasm32-unknown-unknown")
                    .join("release")
                    .join("app.wasm")
            ]
        );

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn deeper_tooling_layouts_stay_out_of_reach() {
        let dir = std::env::temp_dir().join("wasmcheck-nested-in-fallback");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("dist")
                .join("assets")
                .join("app.wasm")
                .display()
                .to_string(),
            b"\0asm stub",
        );

        // Two levels below `dist`, i.e. three below the search dir: past the
        // one-level fallback, by design.
        assert!(find_wasm_files(&dir).expect("find").is_empty());

        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn cargo_debug_layout_is_found_too() {
        let dir = std::env::temp_dir().join("wasmcheck-cargo-debug-layout");
        let _ = fs::remove_dir_all(&dir);
        write(
            &dir.join("target")
                .join("wasm32-unknown-unknown")
                .join("debug")
                .join("app.wasm")
                .display()
                .to_string(),
            b"\0asm stub",
        );

        let found = find_wasm_files(&dir).expect("find");
        assert_eq!(found.len(), 1);

        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
