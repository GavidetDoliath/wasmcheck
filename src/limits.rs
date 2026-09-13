//! Per-metric limits: absolute budgets and growth thresholds.

use serde::{Deserialize, Serialize};

use crate::metric::Metric;
use crate::size::{Size, SizeReport};
use crate::threshold::Threshold;

/// Limits of some kind, one optional value per [`Metric`].
///
/// Limits are validated values, so an invalid one is rejected while the config
/// is read rather than while a bundle is measured.
///
/// # Examples
///
/// ```
/// use wasmcheck::{Budget, Metric, SizeReport};
///
/// let budget = Budget::raw_only("700 KB".parse()?);
/// assert_eq!(budget.limit(Metric::Raw).unwrap().bytes(), 700 * 1024);
/// assert!(budget.limit(Metric::Gzip).is_none());
///
/// let ok = SizeReport::new("app.wasm", 600_000, 200_000, 180_000);
/// assert!(budget.exceeded_metrics(&ok).is_empty());
/// # Ok::<(), wasmcheck::WasmCheckError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits<T> {
    raw: Option<T>,
    gzip: Option<T>,
    brotli: Option<T>,
}

/// Absolute per-metric size caps — the `budget` config key.
pub type Budget = Limits<Size>;

/// Per-metric growth allowances over the committed baseline — the `max_delta`
/// config key. Each threshold is either an absolute size (`"50 KB"`) or a
/// percentage of the baseline (`"5%"`).
pub type MaxDelta = Limits<Threshold>;

impl<T> Limits<T> {
    /// A set of limits that gates nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            raw: None,
            gzip: None,
            brotli: None,
        }
    }

    /// The limit configured for `metric`, if any.
    #[must_use]
    pub const fn limit(&self, metric: Metric) -> Option<&T> {
        match metric {
            Metric::Raw => self.raw.as_ref(),
            Metric::Gzip => self.gzip.as_ref(),
            Metric::Brotli => self.brotli.as_ref(),
        }
    }

    /// Sets (or clears, with `None`) the limit for `metric`.
    pub fn set_limit(&mut self, metric: Metric, limit: Option<T>) {
        match metric {
            Metric::Raw => self.raw = limit,
            Metric::Gzip => self.gzip = limit,
            Metric::Brotli => self.brotli = limit,
        }
    }

    /// `true` when no metric is limited.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.raw.is_none() && self.gzip.is_none() && self.brotli.is_none()
    }

    /// A set of limits where only the raw metric is limited, which is what the
    /// `--budget` and `--max-delta` flags mean.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::{MaxDelta, Metric};
    ///
    /// let max_delta = MaxDelta::raw_only("5%".parse()?);
    /// assert_eq!(max_delta.limit(Metric::Raw).unwrap().to_string(), "5%");
    /// assert!(max_delta.limit(Metric::Gzip).is_none());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    #[must_use]
    pub fn raw_only(limit: T) -> Self {
        let mut limits = Self::new();
        limits.set_limit(Metric::Raw, Some(limit));
        limits
    }
}

impl<T> Default for Limits<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl Limits<Size> {
    /// Every metric whose measured value is strictly greater than its limit.
    ///
    /// The returned metrics are in [`Metric::ALL`] order.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::{Budget, Metric, SizeReport};
    ///
    /// let budget = Budget::raw_only("2 KB".parse()?);
    /// let report = SizeReport::new("app.wasm", 3_000, 500, 400);
    /// assert_eq!(budget.exceeded_metrics(&report), vec![Metric::Raw]);
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    pub fn exceeded_metrics(&self, report: &SizeReport) -> Vec<Metric> {
        Metric::ALL
            .into_iter()
            .filter(|metric| {
                self.limit(*metric)
                    .is_some_and(|limit| metric.value(report) > limit.bytes())
            })
            .collect()
    }
}

impl Limits<Threshold> {
    /// Every metric whose `report` grew by **more than** its allowance compared
    /// to `baseline`.
    ///
    /// This is the regression gate: a bundle may be well inside its budget and
    /// still fail here, for instance after a dependency quietly adds 40 KB.
    /// Shrinking is never a failure, and growth exactly equal to the allowance
    /// is tolerated. The returned metrics are in [`Metric::ALL`] order.
    ///
    /// A percentage allowance is resolved against the *baseline* value of the
    /// same metric.
    ///
    /// # Examples
    ///
    /// ```
    /// use wasmcheck::{MaxDelta, Metric, SizeReport};
    ///
    /// let baseline = SizeReport::new("app.wasm", 10_000, 4_000, 3_500);
    ///
    /// let kb = MaxDelta::raw_only("1 KB".parse()?);
    /// let grown = SizeReport::new("app.wasm", 12_000, 4_000, 3_500);
    /// assert_eq!(kb.exceeded_delta(&grown, &baseline), vec![Metric::Raw]);
    ///
    /// // 5% of 10 000 bytes is 500 bytes, so +600 fails and +400 does not.
    /// let percent = MaxDelta::raw_only("5%".parse()?);
    /// let too_much = SizeReport::new("app.wasm", 10_600, 4_000, 3_500);
    /// assert_eq!(percent.exceeded_delta(&too_much, &baseline), vec![Metric::Raw]);
    /// let fine = SizeReport::new("app.wasm", 10_400, 4_000, 3_500);
    /// assert!(percent.exceeded_delta(&fine, &baseline).is_empty());
    /// # Ok::<(), wasmcheck::WasmCheckError>(())
    /// ```
    #[must_use]
    pub fn exceeded_delta(&self, report: &SizeReport, baseline: &SizeReport) -> Vec<Metric> {
        Metric::ALL
            .into_iter()
            .filter(|metric| {
                self.limit(*metric).is_some_and(|threshold| {
                    metric.value(report) > threshold.allowed(metric.value(baseline))
                })
            })
            .collect()
    }

    /// The largest value allowed for `metric` when `baseline` is the reference.
    #[must_use]
    pub fn allowed(&self, metric: Metric, baseline: &SizeReport) -> Option<u64> {
        self.limit(metric)
            .map(|threshold| threshold.allowed(metric.value(baseline)))
    }
}
