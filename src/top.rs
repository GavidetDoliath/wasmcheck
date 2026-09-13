//! Best-effort ranking of the heaviest functions in a `.wasm` module.

use std::collections::HashMap;
use std::path::Path;

use wasmparser::{KnownCustom, Name, Parser, Payload, TypeRef};

use crate::error::WasmCheckError;

/// The `n` largest functions in the module at `path`, as `(size, name)` pairs
/// sorted by descending size.
///
/// Sizes come from each function body's byte range in the code section. Names
/// come from the wasm **name section**; the function index space includes
/// imported functions, so the offset is accounted for and names line up with
/// the real definitions.
///
/// This is best-effort: many release pipelines strip the name section
/// (`dx bundle`, `wasm-opt`), in which case functions are reported as
/// `func_<index>`. It shines on raw
/// `target/wasm32-unknown-unknown/release/*.wasm` artifacts, which keep names.
///
/// # Errors
///
/// Returns [`WasmCheckError::Io`] when the file cannot be read and
/// [`WasmCheckError::WasmParse`] when it is not a well-formed wasm module.
///
/// # Examples
///
/// ```
/// use wasmcheck::top_functions;
///
/// let ranked = top_functions("fixtures/full.wasm", 3)?;
/// assert!(ranked.len() <= 3);
/// assert!(ranked[0].0 >= ranked[ranked.len() - 1].0);
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
pub fn top_functions(
    path: impl AsRef<Path>,
    n: usize,
) -> Result<Vec<(u64, String)>, WasmCheckError> {
    let data = std::fs::read(path.as_ref()).map_err(WasmCheckError::Io)?;

    let mut imported_functions: u32 = 0;
    let mut body_sizes: Vec<u64> = Vec::new();
    let mut names: HashMap<u32, String> = HashMap::new();

    for payload in Parser::new(0).parse_all(&data) {
        match payload.map_err(|e| WasmCheckError::WasmParse(e.to_string()))? {
            Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    let import = import.map_err(|e| WasmCheckError::WasmParse(e.to_string()))?;
                    if matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                        imported_functions += 1;
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let range = body.range();
                body_sizes.push(range.end - range.start);
            }
            Payload::CustomSection(custom) => {
                if let KnownCustom::Name(name_section) = custom.as_known() {
                    for name in name_section.into_iter() {
                        if let Name::Function(name_map) =
                            name.map_err(|e| WasmCheckError::WasmParse(e.to_string()))?
                        {
                            for naming in name_map.into_iter().flatten() {
                                names.insert(naming.index, naming.name.to_string());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if body_sizes.is_empty() {
        return Ok(Vec::new());
    }

    let mut ranked: Vec<(u64, String)> = body_sizes
        .into_iter()
        .enumerate()
        .map(|(ordinal, size)| {
            // A function's index is its position in the whole function index
            // space, i.e. after every imported function.
            let index = imported_functions + ordinal as u32;
            let name = names
                .get(&index)
                .cloned()
                .unwrap_or_else(|| format!("func_{index}"));
            (size, name)
        })
        .collect();
    ranked.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    ranked.truncate(n);
    Ok(ranked)
}
