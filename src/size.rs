//! Byte sizes: the validated [`Size`] used for budgets, and [`SizeReport`], the
//! raw/gzip/brotli measurements of a single file.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use crate::compression::Compression;
use crate::error::WasmCheckError;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// A non-negative byte count.
///
/// [`Size`] is the type used for budgets. It is parsed from human strings such
/// as `"280 KB"`, `"2.5 MB"` or `"10700 B"`, and it renders back **losslessly**
/// so that a config file survives a load/save round trip byte for byte.
///
/// # Examples
///
/// ```
/// use wasmcheck::Size;
///
/// let size: Size = "280 KB".parse()?;
/// assert_eq!(size.bytes(), 280 * 1024);
/// assert_eq!(size.to_string(), "280 KB");
///
/// // An exact file measurement keeps its exact value.
/// let odd = Size::from_bytes(10_700);
/// assert_eq!(odd.to_string(), "10700 B");
/// assert_eq!("10700 B".parse::<Size>()?, odd);
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Size(u64);

impl Size {
    /// Wraps a raw byte count.
    #[must_use]
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The size in bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }

    /// The smallest whole-KiB size that is greater than or equal to `bytes`.
    ///
    /// This is how `init` derives a starting budget: rounding *up* guarantees
    /// the freshly written config does not fail its own first `check`.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::Size;
    ///
    /// assert_eq!(Size::from_kib_ceil(10_700).bytes(), 11 * 1024);
    /// assert_eq!(Size::from_kib_ceil(10_240).bytes(), 10 * 1024);
    /// assert_eq!(Size::from_kib_ceil(0).bytes(), 0);
    /// ```
    #[must_use]
    pub const fn from_kib_ceil(bytes: u64) -> Self {
        let rem = bytes % KIB;
        if rem == 0 {
            Self(bytes)
        } else {
            Self(bytes - rem + KIB)
        }
    }
}

impl From<u64> for Size {
    fn from(bytes: u64) -> Self {
        Self(bytes)
    }
}

impl From<Size> for u64 {
    fn from(size: Size) -> Self {
        size.0
    }
}

impl fmt::Display for Size {
    /// Formats the size losslessly: `"11 KB"`, `"2 MB"` or `"10700 B"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        if bytes != 0 && bytes % MIB == 0 {
            write!(f, "{} MB", bytes / MIB)
        } else if bytes != 0 && bytes % KIB == 0 {
            write!(f, "{} KB", bytes / KIB)
        } else {
            write!(f, "{bytes} B")
        }
    }
}

impl FromStr for Size {
    type Err = WasmCheckError;

    /// Parses a size such as `"280 KB"`, `"2.5 MB"`, `"512"` or `"10700 B"`.
    ///
    /// Units are case-insensitive and `"B"`, `"KB"`/`"KiB"` and `"MB"`/`"MiB"`
    /// are understood. Negative, non-finite and out-of-range values are
    /// rejected instead of silently saturating.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::InvalidSize`] when the string is not a
    /// non-negative number followed by an optional known unit.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::Size;
    ///
    /// assert_eq!("1.5 KB".parse::<Size>()?.bytes(), 1536);
    /// assert!("-1 KB".parse::<Size>().is_err());
    /// assert!("10 GB".parse::<Size>().is_err());
    /// assert!("nan".parse::<Size>().is_err());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || WasmCheckError::InvalidSize(s.trim().to_string());

        let upper = s.trim().to_ascii_uppercase();
        let (number, unit) = match upper.find(|c: char| c.is_ascii_alphabetic()) {
            Some(pos) => (upper[..pos].trim(), upper[pos..].trim()),
            None => (upper.as_str(), ""),
        };

        let value: f64 = number.parse().map_err(|_| invalid())?;
        if !value.is_finite() || value < 0.0 {
            return Err(invalid());
        }

        let multiplier = match unit {
            "" | "B" => 1.0,
            "KB" | "KIB" => KIB as f64,
            "MB" | "MIB" => MIB as f64,
            _ => return Err(invalid()),
        };

        let bytes = value * multiplier;
        if bytes > u64::MAX as f64 {
            return Err(invalid());
        }
        Ok(Self(bytes as u64))
    }
}

impl serde::Serialize for Size {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Size {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        crate::deserialize::string_or_number(
            deserializer,
            "a byte count or a size string such as \"280 KB\"",
        )
    }
}

/// Formats a byte count for humans: `"675.21 KB"`, `"1.50 MB"`, `"500 B"`.
///
/// Unlike [`Size`]'s `Display`, this is a rounded *presentation* format: it is
/// not meant to be parsed back.
#[must_use]
pub fn format_size(bytes: u64) -> String {
    if bytes >= MIB {
        format!("{:.2} MB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Formats the difference between a measured value and its baseline, e.g.
/// `"+5.64 KB"` or `"±0 B"`.
#[must_use]
pub fn delta_str(value: u64, baseline: u64) -> String {
    match value.cmp(&baseline) {
        std::cmp::Ordering::Greater => format!("+{}", format_size(value - baseline)),
        std::cmp::Ordering::Less => format!("-{}", format_size(baseline - value)),
        std::cmp::Ordering::Equal => "±0 B".to_string(),
    }
}

/// The raw, gzip and brotli size of one bundle, in bytes.
///
/// # Examples
///
/// ```
/// use wasmcheck::{Metric, SizeReport};
///
/// let report = SizeReport::new("app.wasm", 10_000, 4_000, 3_500);
/// assert_eq!(report.value(Metric::Gzip), 4_000);
/// assert_eq!(report.file(), "app.wasm");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SizeReport {
    file: String,
    raw: u64,
    gzip: u64,
    brotli: u64,
}

impl SizeReport {
    /// Builds a report from already-known sizes.
    pub fn new(file: impl Into<String>, raw: u64, gzip: u64, brotli: u64) -> Self {
        Self {
            file: file.into(),
            raw,
            gzip,
            brotli,
        }
    }

    /// Reads `path` and measures its raw, gzip and brotli sizes with
    /// [`Compression::default`] — the CDN preset.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::FileNotFound`] when the file does not exist,
    /// [`WasmCheckError::Io`] for any other read/compression failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wasmcheck::SizeReport;
    ///
    /// let report = SizeReport::measure("dist/app.wasm")?;
    /// println!("{} is {} bytes raw", report.file(), report.raw());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn measure(path: impl AsRef<Path>) -> Result<Self, WasmCheckError> {
        Self::measure_with(path, Compression::default())
    }

    /// Reads `path` and measures its raw, gzip and brotli sizes with an
    /// explicit compression setting.
    ///
    /// The gzip and brotli numbers depend on the setting, not just on the file:
    /// a bundle measured at [`Compression::max`] reports a smaller compressed
    /// size than the same bundle measured at [`Compression::cdn`]. Changing the
    /// setting therefore invalidates a committed gzip/brotli baseline (the raw
    /// size is unaffected).
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::FileNotFound`] when the file does not exist,
    /// [`WasmCheckError::Io`] for any other read/compression failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use wasmcheck::{Compression, SizeReport};
    ///
    /// let report = SizeReport::measure_with("dist/app.wasm", Compression::max())?;
    /// println!("gzip {} bytes", report.gzip());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn measure_with(
        path: impl AsRef<Path>,
        compression: Compression,
    ) -> Result<Self, WasmCheckError> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                WasmCheckError::FileNotFound(path.to_path_buf())
            } else {
                WasmCheckError::Io(e)
            }
        })?;

        Ok(Self {
            file: path.display().to_string(),
            raw: data.len() as u64,
            gzip: compress_gzip(&data, compression.gzip_level())?,
            brotli: compress_brotli(&data, compression.brotli_quality())?,
        })
    }

    /// The path this report was measured from.
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// The uncompressed size in bytes.
    #[must_use]
    pub const fn raw(&self) -> u64 {
        self.raw
    }

    /// The gzip-compressed size in bytes.
    #[must_use]
    pub const fn gzip(&self) -> u64 {
        self.gzip
    }

    /// The brotli-compressed size in bytes.
    #[must_use]
    pub const fn brotli(&self) -> u64 {
        self.brotli
    }

    /// The value of `metric` for this report.
    #[must_use]
    pub fn value(&self, metric: crate::Metric) -> u64 {
        metric.value(self)
    }

    /// The signed difference between this report and `baseline`, per metric.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::{Metric, SizeReport};
    ///
    /// let before = SizeReport::new("app.wasm", 1000, 500, 400);
    /// let after = SizeReport::new("app.wasm", 1200, 480, 400);
    ///
    /// let delta = after.delta(&before);
    /// assert_eq!(delta.get(Metric::Raw), 200);
    /// assert_eq!(delta.get(Metric::Gzip), -20);
    /// assert_eq!(delta.get(Metric::Brotli), 0);
    /// ```
    #[must_use]
    pub fn delta(&self, baseline: &Self) -> SizeDelta {
        SizeDelta {
            raw: self.raw as i64 - baseline.raw as i64,
            gzip: self.gzip as i64 - baseline.gzip as i64,
            brotli: self.brotli as i64 - baseline.brotli as i64,
        }
    }
}

/// The signed difference between a measurement and its baseline, in bytes.
///
/// Positive values mean the bundle grew, negative values mean it shrank. This
/// is the machine-readable counterpart of [`delta_str`], which is meant for the
/// table output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SizeDelta {
    raw: i64,
    gzip: i64,
    brotli: i64,
}

impl SizeDelta {
    /// The difference for `metric`, in bytes.
    #[must_use]
    pub const fn get(&self, metric: crate::Metric) -> i64 {
        match metric {
            crate::Metric::Raw => self.raw,
            crate::Metric::Gzip => self.gzip,
            crate::Metric::Brotli => self.brotli,
        }
    }
}

fn compress_gzip(data: &[u8], level: u32) -> Result<u64, WasmCheckError> {
    use std::io::Write;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(level));
    encoder.write_all(data)?;
    Ok(encoder.finish()?.len() as u64)
}

fn compress_brotli(data: &[u8], quality: i32) -> Result<u64, WasmCheckError> {
    // `quality` drives the hasher selection inside the encoder, so the other
    // fields can keep their defaults.
    let params = brotli::enc::BrotliEncoderParams {
        quality,
        ..brotli::enc::BrotliEncoderParams::default()
    };

    let mut output = Vec::new();
    let mut input = data;
    brotli::BrotliCompress(&mut input, &mut output, &params)?;
    Ok(output.len() as u64)
}
