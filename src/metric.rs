//! The three size metrics wasmcheck reports on.

use std::fmt;
use std::str::FromStr;

use crate::error::WasmCheckError;
use crate::size::SizeReport;

/// One of the three sizes measured for every bundle.
///
/// # Examples
///
/// ```
/// use wasmcheck::{Metric, SizeReport};
///
/// let report = SizeReport::new("app.wasm", 700_000, 250_000, 200_000);
/// assert_eq!(Metric::Gzip.value(&report), 250_000);
/// assert_eq!(Metric::Gzip.to_string(), "gzip");
/// assert_eq!("brotli".parse::<Metric>()?, Metric::Brotli);
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Metric {
    /// The uncompressed bundle size.
    Raw,
    /// The size after gzip compression.
    Gzip,
    /// The size after brotli compression.
    Brotli,
}

impl Metric {
    /// Every metric, in the order they are reported.
    pub const ALL: [Metric; 3] = [Metric::Raw, Metric::Gzip, Metric::Brotli];

    /// The measured value of this metric in `report`.
    #[must_use]
    pub fn value(self, report: &SizeReport) -> u64 {
        match self {
            Metric::Raw => report.raw(),
            Metric::Gzip => report.gzip(),
            Metric::Brotli => report.brotli(),
        }
    }
}

impl fmt::Display for Metric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Metric::Raw => "raw",
            Metric::Gzip => "gzip",
            Metric::Brotli => "brotli",
        })
    }
}

impl FromStr for Metric {
    type Err = WasmCheckError;

    /// Parses `"raw"`, `"gzip"` or `"brotli"` (case-insensitive).
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::InvalidMetric`] for any other name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "raw" => Ok(Metric::Raw),
            "gzip" => Ok(Metric::Gzip),
            "brotli" => Ok(Metric::Brotli),
            other => Err(WasmCheckError::InvalidMetric(other.to_string())),
        }
    }
}
