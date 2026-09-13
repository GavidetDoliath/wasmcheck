#[derive(Debug, thiserror::Error)]
pub enum WasmCheckError {
    #[error("No .wasm file found in current directory")]
    NoWasmFound,
    #[error("Multiple .wasm files found: {found:?}. Use --file to specify one")]
    MultipleWasmFound { found: Vec<String> },
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SizeReport {
    pub file: String,
    pub raw: u64,
    pub gzip: u64,
    pub brotli: u64,
}

impl SizeReport {
    pub fn measure(path: &str) -> Result<Self, WasmCheckError> {
        let data = std::fs::read(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                WasmCheckError::FileNotFound(path.to_string())
            } else {
                WasmCheckError::Io(e)
            }
        })?;

        let raw = data.len() as u64;
        let gzip = compress_gzip(&data);
        let brotli = compress_brotli(&data);

        Ok(Self {
            file: path.to_string(),
            raw,
            gzip,
            brotli,
        })
    }
}

fn compress_gzip(data: &[u8]) -> u64 {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap().len() as u64
}

fn compress_brotli(data: &[u8]) -> u64 {
    let mut output = Vec::new();
    let mut data = data;
    brotli::BrotliCompress(
        &mut data,
        &mut output,
        &brotli::enc::BrotliEncoderParams::default(),
    )
    .unwrap();
    output.len() as u64
}

pub fn find_wasm_files(dir: &str) -> Result<Vec<String>, WasmCheckError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(WasmCheckError::Io)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "wasm")
                .unwrap_or(false)
        })
        .map(|e| e.path().to_string_lossy().to_string())
        .collect();
    entries.sort();
    Ok(entries)
}

pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_uppercase();
    let (num, unit) = if let Some(pos) = s.find(|c: char| c.is_alphabetic()) {
        (s[..pos].trim(), s[pos..].trim())
    } else {
        (s.as_str(), "")
    };

    let value: f64 = num
        .parse()
        .map_err(|_| format!("invalid number: `{}`", &s[..num.len()]))?;

    match unit {
        "B" | "" => Ok(value as u64),
        "KB" => Ok((value * 1024.0) as u64),
        "MB" => Ok((value * 1024.0 * 1024.0) as u64),
        _ => Err(format!("unknown unit: `{unit}` (use B, KB, MB)")),
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size_bytes() {
        assert_eq!(parse_size("500B").unwrap(), 500);
        assert_eq!(parse_size("500 B").unwrap(), 500);
    }

    #[test]
    fn test_parse_size_kb() {
        assert_eq!(parse_size("1.5KB").unwrap(), 1536);
        assert_eq!(parse_size("10 KB").unwrap(), 10240);
    }

    #[test]
    fn test_parse_size_mb() {
        assert_eq!(parse_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(
            parse_size("2.5 MB").unwrap(),
            (2.5 * 1024.0 * 1024.0) as u64
        );
    }

    #[test]
    fn test_parse_size_invalid() {
        assert!(parse_size("abc").is_err());
        assert!(parse_size("10GB").is_err());
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
    }

    #[test]
    fn test_compress_gzip_not_empty() {
        let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let data = data.repeat(8);
        let compressed = compress_gzip(&data);
        assert!(compressed > 0);
        assert!((compressed as usize) < data.len()); // should compress
    }

    #[test]
    fn test_compress_brotli_not_empty() {
        let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let data = data.repeat(8);
        let compressed = compress_brotli(&data);
        assert!(compressed > 0);
        assert!((compressed as usize) < data.len()); // should compress
    }

    #[test]
    fn test_find_wasm_files_empty_dir() {
        let dir = std::env::temp_dir().join("wasmcheck_test_empty");
        std::fs::create_dir_all(&dir).unwrap();
        let result = find_wasm_files(dir.to_str().unwrap()).unwrap();
        assert!(result.is_empty());
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn test_find_wasm_files_with_fixtures() {
        let dir = std::env::temp_dir().join("wasmcheck_test_fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.wasm"), b"fake wasm").unwrap();
        std::fs::write(dir.join("other.txt"), b"not wasm").unwrap();
        let result = find_wasm_files(dir.to_str().unwrap()).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].ends_with("test.wasm"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_measure_fixture() {
        let dir = std::env::temp_dir().join("wasmcheck_test_measure");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.wasm");
        let content = b"hello world, this is some test wasm content that repeats to make it compressible. hello world, this is some test wasm content that repeats to make it compressible.";
        std::fs::write(&path, content).unwrap();

        let report = SizeReport::measure(path.to_str().unwrap()).unwrap();
        assert_eq!(report.raw, content.len() as u64);
        assert!(report.gzip > 0);
        assert!(report.brotli > 0);
        assert!(report.gzip < report.raw); // compressed smaller
        assert!(report.brotli < report.raw);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
