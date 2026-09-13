use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum WasmCheckError {
    #[error("No .wasm file found in current directory")]
    NoWasmFound,
    #[error("Multiple .wasm files found: {found:?}. Use --file to specify one")]
    MultipleWasmFound { found: Vec<String> },
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("Config not found: {0}")]
    ConfigNotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse {path}: {source}")]
    ConfigParse {
        path: String,
        source: serde_json::Error,
    },
    #[error("Invalid size `{0}` (use e.g. \"280 KB\")")]
    InvalidSize(String),
    #[error("Budget exceeded: {0}")]
    BudgetExceeded(String),
    #[error("Failed to serialize config: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub const CONFIG_FILE: &str = ".wasmcheck.json";

impl WasmCheckError {
    pub fn is_budget_exceeded(&self) -> bool {
        matches!(self, WasmCheckError::BudgetExceeded(_))
    }
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

pub fn parse_size(s: &str) -> Result<u64, WasmCheckError> {
    let s = s.trim().to_uppercase();
    let (num, unit) = if let Some(pos) = s.find(|c: char| c.is_alphabetic()) {
        (s[..pos].trim(), s[pos..].trim())
    } else {
        (s.as_str(), "")
    };

    let value: f64 = num
        .parse()
        .map_err(|_| WasmCheckError::InvalidSize(s.to_string()))?;

    match unit {
        "B" | "" => Ok(value as u64),
        "KB" => Ok((value * 1024.0) as u64),
        "MB" => Ok((value * 1024.0 * 1024.0) as u64),
        _ => Err(WasmCheckError::InvalidSize(s.to_string())),
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

#[derive(Debug, Clone)]
pub enum Metric {
    Raw,
    Gzip,
    Brotli,
}

impl Metric {
    pub const ALL: [Metric; 3] = [Metric::Raw, Metric::Gzip, Metric::Brotli];

    pub const fn label(&self) -> &'static str {
        match self {
            Metric::Raw => "raw",
            Metric::Gzip => "gzip",
            Metric::Brotli => "brotli",
        }
    }

    pub const fn value(&self, report: &SizeReport) -> u64 {
        match self {
            Metric::Raw => report.raw,
            Metric::Gzip => report.gzip,
            Metric::Brotli => report.brotli,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Budget {
    pub raw: Option<String>,
    pub gzip: Option<String>,
    pub brotli: Option<String>,
}

impl Budget {
    pub fn from_raw(raw_str: String) -> Result<Self, WasmCheckError> {
        parse_size(&raw_str)?;
        Ok(Self {
            raw: Some(raw_str),
            gzip: None,
            brotli: None,
        })
    }

    pub fn limits_bytes(&self) -> Result<[Option<u64>; 3], WasmCheckError> {
        let mut limits: Vec<Option<u64>> = Vec::with_capacity(3);
        for s in [&self.raw, &self.gzip, &self.brotli] {
            match s {
                Some(s) => limits.push(Some(parse_size(s)?)),
                None => limits.push(None),
            }
        }
        Ok([limits[0], limits[1], limits[2]])
    }

    pub fn exceeded_metrics(&self, report: &SizeReport) -> Result<Vec<Metric>, WasmCheckError> {
        let limits = self.limits_bytes()?;
        let mut exceeded = Vec::new();
        for (i, metric) in Metric::ALL.iter().enumerate() {
            if let Some(limit) = limits[i]
                && metric.value(report) > limit
            {
                exceeded.push(metric.clone());
            }
        }
        Ok(exceeded)
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    /// Exact file paths (relative to config location) to check.
    /// Empty = auto-detect .wasm files in the config directory.
    pub files: Vec<String>,
    pub budget: Option<Budget>,
    /// Committed baseline, keyed by file path.
    pub baseline: BTreeMap<String, SizeReport>,
}

impl Config {
    pub fn load(path: &str) -> Result<Self, WasmCheckError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                WasmCheckError::ConfigNotFound(path.to_string())
            } else {
                WasmCheckError::Io(e)
            }
        })?;
        let config = serde_json::from_str(&text).map_err(|source| WasmCheckError::ConfigParse {
            path: path.to_string(),
            source,
        })?;
        Ok(config)
    }

    pub fn save(&self, path: &str) -> Result<(), WasmCheckError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).map_err(WasmCheckError::Io)
    }
}

pub fn delta_str(value: u64, baseline: u64) -> String {
    if value > baseline {
        format!("+{}", format_size(value - baseline))
    } else if value < baseline {
        format!("-{}", format_size(baseline - value))
    } else {
        "±0 B".to_string()
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
        assert!((compressed as usize) < data.len());
    }

    #[test]
    fn test_compress_brotli_not_empty() {
        let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let data = data.repeat(8);
        let compressed = compress_brotli(&data);
        assert!(compressed > 0);
        assert!((compressed as usize) < data.len());
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
        assert!(report.gzip < report.raw);
        assert!(report.brotli < report.raw);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_budget_pass() {
        let report = SizeReport {
            file: "x".into(),
            raw: 1000,
            gzip: 500,
            brotli: 400,
        };
        let budget = Budget {
            raw: Some("2 KB".into()),
            gzip: None,
            brotli: None,
        };
        assert!(budget.exceeded_metrics(&report).unwrap().is_empty());
    }

    #[test]
    fn test_budget_exceeded_raw() {
        let report = SizeReport {
            file: "x".into(),
            raw: 3000,
            gzip: 500,
            brotli: 400,
        };
        let budget = Budget {
            raw: Some("2 KB".into()),
            gzip: None,
            brotli: None,
        };
        let exceeded = budget.exceeded_metrics(&report).unwrap();
        assert_eq!(exceeded.len(), 1);
        assert_eq!(exceeded[0].label(), "raw");
    }

    #[test]
    fn test_config_roundtrip() {
        let dir = std::env::temp_dir().join("wasmcheck_test_config_roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);
        let config = Config {
            files: vec!["dist/app.wasm".into()],
            budget: Some(Budget {
                raw: Some("280 KB".into()),
                gzip: Some("80 KB".into()),
                brotli: None,
            }),
            baseline: BTreeMap::new(),
        };
        config.save(path.to_str().unwrap()).unwrap();

        let loaded = Config::load(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.files, vec!["dist/app.wasm"]);
        assert_eq!(loaded.budget.unwrap().raw.unwrap(), "280 KB");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_config_load_missing() {
        let result = Config::load("/nonexistent/.wasmcheck.json");
        assert!(matches!(result, Err(WasmCheckError::ConfigNotFound(_))));
    }

    #[test]
    fn test_delta_str() {
        assert_eq!(delta_str(1000, 1024), "-24 B");
        assert_eq!(delta_str(1100, 1024), "+76 B");
        assert_eq!(delta_str(1024, 1024), "±0 B");
    }
}
