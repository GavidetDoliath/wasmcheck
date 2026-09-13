//! Growth thresholds: absolute sizes, or percentages of the baseline.

use std::fmt;
use std::str::FromStr;

use crate::deserialize;
use crate::error::WasmCheckError;
use crate::size::Size;

/// A percentage with up to two decimal places, stored in hundredths of a
/// percent — `5%` is `500`, `0.5%` is `50`.
///
/// # Examples
///
/// ```
/// use wasmcheck::Percent;
///
/// let five: Percent = "5%".parse()?;
/// assert_eq!(five.hundredths(), 500);
/// assert_eq!(five.to_string(), "5%");
/// assert_eq!(five.of(675_212), 33_760);
///
/// assert_eq!("0.5%".parse::<Percent>()?.to_string(), "0.5%");
/// assert_eq!("12.25%".parse::<Percent>()?.to_string(), "12.25%");
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Percent(u32);

impl Percent {
    /// Wraps a value expressed in hundredths of a percent (`500` = `5%`).
    #[must_use]
    pub const fn from_hundredths(hundredths: u32) -> Self {
        Self(hundredths)
    }

    /// The value in hundredths of a percent.
    #[must_use]
    pub const fn hundredths(self) -> u32 {
        self.0
    }

    /// `self` percent of `baseline`, truncated to whole bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::Percent;
    ///
    /// assert_eq!(Percent::from_hundredths(500).of(100_000), 5_000);
    /// assert_eq!(Percent::from_hundredths(100).of(100_000), 1_000);
    /// assert_eq!(Percent::from_hundredths(1).of(100_000), 10);
    /// ```
    #[must_use]
    pub const fn of(self, baseline: u64) -> u64 {
        let scaled = baseline as u128 * self.0 as u128;
        let bytes = scaled / 10_000;
        if bytes > u64::MAX as u128 {
            u64::MAX
        } else {
            bytes as u64
        }
    }
}

impl fmt::Display for Percent {
    /// Formats the minimal exact form: `"5%"`, `"0.5%"`, `"12.25%"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hundredths = self.0;
        let whole = hundredths / 100;
        let fraction = hundredths % 100;
        if fraction == 0 {
            write!(f, "{whole}%")
        } else if fraction.is_multiple_of(10) {
            write!(f, "{whole}.{}%", fraction / 10)
        } else {
            write!(f, "{whole}.{fraction:02}%")
        }
    }
}

impl FromStr for Percent {
    type Err = WasmCheckError;

    /// Parses `"5%"`, `"0.5%"` or a bare `"5"` (the `%` is implied).
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::InvalidPercent`] when the value is not a
    /// finite, non-negative percentage that fits two decimal places.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || WasmCheckError::InvalidPercent(s.trim().to_string());
        let number = s.trim().strip_suffix('%').unwrap_or(s.trim()).trim();

        let value: f64 = number.parse().map_err(|_| invalid())?;
        if !value.is_finite() || value < 0.0 {
            return Err(invalid());
        }

        let hundredths = value * 100.0;
        if hundredths > u32::MAX as f64 {
            return Err(invalid());
        }
        Ok(Self(hundredths.round() as u32))
    }
}

/// How much a bundle is allowed to grow over its baseline.
///
/// # Examples
///
/// ```
/// use wasmcheck::Threshold;
///
/// let absolute: Threshold = "50 KB".parse()?;
/// assert_eq!(absolute.allowed(1_000_000), 1_000_000 + 50 * 1024);
///
/// let relative: Threshold = "5%".parse()?;
/// assert_eq!(relative.allowed(1_000_000), 1_050_000);
/// assert_eq!(relative.to_string(), "5%");
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Threshold {
    /// An absolute allowance, in bytes.
    Bytes(Size),
    /// An allowance relative to the baseline.
    Percent(Percent),
}

impl Threshold {
    /// The largest value allowed when `baseline` is the reference.
    #[must_use]
    pub const fn allowed(self, baseline: u64) -> u64 {
        match self {
            Threshold::Bytes(bytes) => baseline.saturating_add(bytes.bytes()),
            Threshold::Percent(percent) => baseline.saturating_add(percent.of(baseline)),
        }
    }
}

impl fmt::Display for Threshold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Threshold::Bytes(bytes) => bytes.fmt(f),
            Threshold::Percent(percent) => percent.fmt(f),
        }
    }
}

impl FromStr for Threshold {
    type Err = WasmCheckError;

    /// Parses `"50 KB"` as an absolute allowance and `"5%"` as a relative one.
    ///
    /// # Errors
    ///
    /// Returns [`WasmCheckError::InvalidSize`] or
    /// [`WasmCheckError::InvalidPercent`] depending on the suffix.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().ends_with('%') {
            s.parse().map(Threshold::Percent)
        } else {
            s.parse().map(Threshold::Bytes)
        }
    }
}

impl serde::Serialize for Threshold {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Threshold {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize::string_or_number(
            deserializer,
            "a byte count or a threshold string such as \"50 KB\" or \"5%\"",
        )
    }
}
