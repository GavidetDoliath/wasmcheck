//! How hard `wasmcheck` compresses before reporting a size.

use std::fmt;

use crate::error::WasmCheckError;

/// The gzip level and brotli quality used to measure a bundle.
///
/// The defaults model **what a server or CDN does on the fly** — gzip level 6,
/// brotli quality 5 — rather than the best compression achievable. Measuring at
/// the maximum reports a size smaller than what actually goes over the wire, so
/// a budget can pass here and still be blown in production.
///
/// Use [`Compression::max`] when your assets are precompressed at build time
/// and served as-is.
///
/// # Examples
///
/// ```
/// use wasmcheck::Compression;
///
/// // The default is the CDN preset.
/// assert_eq!(Compression::default(), Compression::cdn());
/// assert_eq!(Compression::cdn().gzip_level(), 6);
/// assert_eq!(Compression::cdn().brotli_quality(), 5);
///
/// // Precompressed assets:
/// assert_eq!(Compression::max(), Compression::new(9, 11)?);
///
/// assert!(Compression::new(10, 5).is_err());
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct Compression {
    gzip_level: u32,
    brotli_quality: i32,
}

impl Compression {
    /// The gzip level [`Compression::cdn`] uses.
    pub const CDN_GZIP_LEVEL: u32 = 6;
    /// The brotli quality [`Compression::cdn`] uses.
    pub const CDN_BROTLI_QUALITY: i32 = 5;
    /// The gzip level [`Compression::max`] uses.
    pub const MAX_GZIP_LEVEL: u32 = 9;
    /// The brotli quality [`Compression::max`] uses.
    pub const MAX_BROTLI_QUALITY: i32 = 11;

    /// What a server or CDN does on the fly: gzip 6, brotli 5. This is the
    /// default.
    #[must_use]
    pub const fn cdn() -> Self {
        Self {
            gzip_level: Self::CDN_GZIP_LEVEL,
            brotli_quality: Self::CDN_BROTLI_QUALITY,
        }
    }

    /// Precompression at the highest setting: gzip 9, brotli 11.
    #[must_use]
    pub const fn max() -> Self {
        Self {
            gzip_level: Self::MAX_GZIP_LEVEL,
            brotli_quality: Self::MAX_BROTLI_QUALITY,
        }
    }

    /// A custom setting.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::InvalidGzipLevel`] when `gzip_level` is above
    /// 9 and [`WasmCheckError::InvalidBrotliQuality`] when `brotli_quality` is
    /// outside `0..=11`.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::Compression;
    ///
    /// assert_eq!(Compression::new(9, 11)?.gzip_level(), 9);
    /// assert!(Compression::new(5, 12).is_err());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn new(gzip_level: u32, brotli_quality: i32) -> Result<Self, WasmCheckError> {
        let compression = Self {
            gzip_level,
            brotli_quality,
        };
        compression.validate()?;
        Ok(compression)
    }

    /// The gzip compression level, `0..=9`.
    #[must_use]
    pub const fn gzip_level(&self) -> u32 {
        self.gzip_level
    }

    /// The brotli quality, `0..=11`.
    #[must_use]
    pub const fn brotli_quality(&self) -> i32 {
        self.brotli_quality
    }

    /// The same setting with a different gzip level.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::InvalidGzipLevel`] when `level` is above 9.
    pub fn with_gzip_level(self, level: u32) -> Result<Self, WasmCheckError> {
        Self::new(level, self.brotli_quality)
    }

    /// The same setting with a different brotli quality.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::InvalidBrotliQuality`] when `quality` is
    /// outside `0..=11`.
    pub fn with_brotli_quality(self, quality: i32) -> Result<Self, WasmCheckError> {
        Self::new(self.gzip_level, quality)
    }

    /// `true` when this is the [`Compression::cdn`] preset, i.e. the default.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.gzip_level == Self::CDN_GZIP_LEVEL && self.brotli_quality == Self::CDN_BROTLI_QUALITY
    }

    fn validate(&self) -> Result<(), WasmCheckError> {
        if self.gzip_level > Self::MAX_GZIP_LEVEL {
            return Err(WasmCheckError::InvalidGzipLevel(i64::from(self.gzip_level)));
        }
        if self.brotli_quality < 0 || self.brotli_quality > Self::MAX_BROTLI_QUALITY {
            return Err(WasmCheckError::InvalidBrotliQuality(i64::from(
                self.brotli_quality,
            )));
        }
        Ok(())
    }
}

impl Default for Compression {
    fn default() -> Self {
        Self::cdn()
    }
}

impl fmt::Display for Compression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "gzip level {}, brotli quality {}",
            self.gzip_level, self.brotli_quality
        )
    }
}

impl<'de> serde::Deserialize<'de> for Compression {
    /// Deserializes `{"gzip_level": 6, "brotli_quality": 5}`, defaulting each
    /// missing field to the CDN preset and rejecting out-of-range values.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Raw {
            gzip_level: Option<u32>,
            brotli_quality: Option<i32>,
        }

        let raw = Raw::deserialize(deserializer)?;
        let cdn = Compression::cdn();
        Compression::new(
            raw.gzip_level.unwrap_or(cdn.gzip_level()),
            raw.brotli_quality.unwrap_or(cdn.brotli_quality()),
        )
        .map_err(serde::de::Error::custom)
    }
}
