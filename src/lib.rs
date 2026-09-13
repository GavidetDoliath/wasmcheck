//! A WASM bundle size budget checker.
//!
//! `wasmcheck` measures the **raw**, **gzip** and **brotli** size of `.wasm`
//! bundles, keeps a committed baseline with deltas, and fails CI when a budget
//! is exceeded. It is the WASM equivalent of `size-limit`, not a profiler:
//! `twiggy` explains *why* a binary is big, `wasmcheck` decides whether you may
//! merge it.
//!
//! # Example
//!
//! ```
//! use wasmcheck::{Budget, Metric, SizeReport};
//!
//! let mut budget = Budget::new();
//! budget.set_limit(Metric::Gzip, Some("250 KB".parse()?));
//!
//! let report = SizeReport::new("app.wasm", 675_212, 240_928, 205_110);
//! assert!(budget.exceeded_metrics(&report).is_empty());
//! # Ok::<(), wasmcheck::WasmCheckError>(())
//! ```
//!
//! Sizes are given as strings such as `"280 KB"` or `"2.5 MB"` and parsed into
//! a validated [`Size`], so a bad budget is rejected while the config is read
//! instead of while a bundle is measured.
//!
//! # Exit codes
//!
//! The `wasmcheck` binary exits with `0` when every file is under budget and
//! `1` when a budget is exceeded or an error occurred. Use
//! [`WasmCheckError::is_budget_exceeded`] to tell those two cases apart when
//! embedding the library.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod baseline;
mod compression;
mod config;
mod deserialize;
mod error;
mod files;
mod limits;
mod metric;
mod size;
mod threshold;
mod top;

pub use baseline::Baseline;
pub use compression::Compression;
pub use config::Config;
pub use error::WasmCheckError;
pub use files::{ResolvedFile, find_wasm_files, resolve_files};
pub use limits::{Budget, Limits, MaxDelta};
pub use metric::Metric;
pub use size::{Size, SizeDelta, SizeReport, delta_str, format_size};
pub use threshold::{Percent, Threshold};
pub use top::top_functions;

/// The config file name `wasmcheck` looks for when `--config` is not given.
pub const CONFIG_FILE: &str = ".wasmcheck.json";

/// The baseline file name used when the config does not name one.
///
/// It sits next to the config so that a rebuild only ever rewrites the baseline
/// file, leaving the hand-edited config conflict-free in pull requests.
pub const DEFAULT_BASELINE_FILE: &str = ".wasmcheck.baseline.json";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A uniquely named temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir()
                .join("wasmcheck_unit")
                .join(format!("{name}_{n}_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    // ---- Size parsing -----------------------------------------------------

    #[test]
    fn parses_units() {
        assert_eq!("500B".parse::<Size>().unwrap().bytes(), 500);
        assert_eq!("500 B".parse::<Size>().unwrap().bytes(), 500);
        assert_eq!("512".parse::<Size>().unwrap().bytes(), 512);
        assert_eq!("1.5KB".parse::<Size>().unwrap().bytes(), 1536);
        assert_eq!("10 KB".parse::<Size>().unwrap().bytes(), 10 * 1024);
        assert_eq!("1MB".parse::<Size>().unwrap().bytes(), 1024 * 1024);
        assert_eq!("2.5 mb".parse::<Size>().unwrap().bytes(), 2_621_440);
    }

    /// Regression: `parse_size("-1 KB")` used to saturate to `0` bytes, so an
    /// obviously wrong budget silently became "everything fails".
    #[test]
    fn rejects_negative_and_non_finite_sizes() {
        for input in ["-1 KB", "-1", "-0.5 MB", "nan", "inf", "abc", "10GB", ""] {
            assert!(
                input.parse::<Size>().is_err(),
                "expected `{input}` to be rejected"
            );
        }
    }

    #[test]
    fn size_display_is_lossless() {
        assert_eq!(Size::from_bytes(10 * 1024).to_string(), "10 KB");
        assert_eq!(Size::from_bytes(2 * 1024 * 1024).to_string(), "2 MB");
        assert_eq!(Size::from_bytes(10_700).to_string(), "10700 B");
        assert_eq!(Size::from_bytes(0).to_string(), "0 B");

        for bytes in [0, 1, 1023, 1024, 1536, 10_700, 700 * 1024, 3 * 1024 * 1024] {
            let size = Size::from_bytes(bytes);
            assert_eq!(size.to_string().parse::<Size>().unwrap(), size);
        }
    }

    /// Regression seed for the `init` budget bug: rounding to the nearest KiB
    /// produced a budget smaller than the file it was derived from.
    #[test]
    fn ceil_kib_never_undershoots() {
        assert_eq!(Size::from_kib_ceil(10_700).bytes(), 11 * 1024);
        assert_eq!(Size::from_kib_ceil(10_240).bytes(), 10 * 1024);
        assert_eq!(Size::from_kib_ceil(1).bytes(), 1024);
        assert_eq!(Size::from_kib_ceil(0).bytes(), 0);
        for bytes in [1u64, 1023, 1024, 10_700, 10_240, 1_000_000] {
            assert!(Size::from_kib_ceil(bytes).bytes() >= bytes);
        }
    }

    #[test]
    fn formats_and_deltas() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(delta_str(1000, 1024), "-24 B");
        assert_eq!(delta_str(1100, 1024), "+76 B");
        assert_eq!(delta_str(1024, 1024), "±0 B");
    }

    // ---- Budgets ----------------------------------------------------------

    #[test]
    fn budget_uses_typed_limits() {
        let mut budget = Budget::new();
        assert!(budget.is_empty());
        budget.set_limit(Metric::Raw, Some("2 KB".parse().unwrap()));
        assert!(!budget.is_empty());
        assert_eq!(budget.limit(Metric::Raw).unwrap().bytes(), 2048);
        assert_eq!(budget.limit(Metric::Gzip), None);
    }

    #[test]
    fn budget_passes_under_limit() {
        let mut budget = Budget::new();
        budget.set_limit(Metric::Raw, Some("2 KB".parse().unwrap()));
        let report = SizeReport::new("x", 1000, 500, 400);
        assert!(budget.exceeded_metrics(&report).is_empty());
    }

    #[test]
    fn budget_reports_every_exceeded_metric_in_order() {
        let mut budget = Budget::new();
        budget.set_limit(Metric::Raw, Some("2 KB".parse().unwrap()));
        budget.set_limit(Metric::Brotli, Some("100 B".parse().unwrap()));
        let report = SizeReport::new("x", 3000, 500, 400);
        assert_eq!(
            budget.exceeded_metrics(&report),
            vec![Metric::Raw, Metric::Brotli]
        );
    }

    #[test]
    fn max_delta_flags_growth_and_ignores_shrinking() {
        let mut max_delta = MaxDelta::new();
        max_delta.set_limit(Metric::Raw, Some("1 KB".parse().unwrap()));
        let baseline = SizeReport::new("app.wasm", 10_000, 4_000, 3_500);

        // Growth of exactly the allowance is tolerated.
        let at_limit = SizeReport::new("app.wasm", 11_024, 4_000, 3_500);
        assert!(max_delta.exceeded_delta(&at_limit, &baseline).is_empty());

        // One byte more is not.
        let over = SizeReport::new("app.wasm", 11_025, 4_000, 3_500);
        assert_eq!(
            max_delta.exceeded_delta(&over, &baseline),
            vec![Metric::Raw]
        );

        // Shrinking never fails, however tight the allowance.
        let shrunk = SizeReport::new("app.wasm", 5, 4, 3);
        assert!(max_delta.exceeded_delta(&shrunk, &baseline).is_empty());
    }

    #[test]
    fn max_delta_covers_every_metric() {
        let mut max_delta = MaxDelta::new();
        max_delta.set_limit(Metric::Gzip, Some("100 B".parse().unwrap()));
        let baseline = SizeReport::new("app.wasm", 10_000, 4_000, 3_500);

        let grew = SizeReport::new("app.wasm", 10_000, 4_500, 3_500);
        assert_eq!(
            max_delta.exceeded_delta(&grew, &baseline),
            vec![Metric::Gzip]
        );

        // A limit on gzip says nothing about the raw size.
        let raw_only = SizeReport::new("app.wasm", 90_000, 4_000, 3_500);
        assert!(max_delta.exceeded_delta(&raw_only, &baseline).is_empty());
    }

    #[test]
    fn config_round_trips_max_delta() {
        let dir = TempDir::new("config_max_delta");
        let path = dir.path().join(CONFIG_FILE);

        let mut config = Config::new();
        config.add_file("app.wasm");
        let mut max_delta = MaxDelta::new();
        max_delta.set_limit(Metric::Raw, Some("50 KB".parse().unwrap()));
        config.set_max_delta(Some(max_delta.clone()));
        config.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"max_delta\""), "{text}");
        assert!(text.contains("\"50 KB\""), "{text}");
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, config);
        assert_eq!(
            loaded
                .max_delta()
                .unwrap()
                .limit(Metric::Raw)
                .unwrap()
                .to_string(),
            "50 KB"
        );
    }

    #[test]
    fn raw_only_limits_the_raw_metric() {
        let budget = Budget::raw_only("280 KB".parse().unwrap());
        assert_eq!(budget.limit(Metric::Raw).unwrap().bytes(), 280 * 1024);
        assert!(budget.limit(Metric::Gzip).is_none());

        let max_delta = MaxDelta::raw_only("5%".parse().unwrap());
        assert_eq!(max_delta.limit(Metric::Raw).unwrap().to_string(), "5%");
        assert!(max_delta.limit(Metric::Gzip).is_none());
    }

    #[test]
    fn percent_parses_and_round_trips() {
        assert_eq!("5%".parse::<Percent>().unwrap().hundredths(), 500);
        assert_eq!("0.5%".parse::<Percent>().unwrap().hundredths(), 50);
        assert_eq!("12.25%".parse::<Percent>().unwrap().hundredths(), 1225);
        // A bare number is read as a percentage too.
        assert_eq!("7".parse::<Percent>().unwrap().hundredths(), 700);

        for text in ["5%", "0.5%", "12.25%", "0.01%", "100%"] {
            let percent = text.parse::<Percent>().unwrap();
            assert_eq!(percent.to_string(), text);
        }

        for input in ["-5%", "abc%", "nan%", "inf%", "%"] {
            assert!(input.parse::<Percent>().is_err(), "{input}");
        }
    }

    #[test]
    fn percent_of_a_baseline_truncates() {
        assert_eq!(Percent::from_hundredths(500).of(100_000), 5_000);
        assert_eq!(Percent::from_hundredths(100).of(100_000), 1_000);
        assert_eq!(Percent::from_hundredths(1).of(100_000), 10);
        // 5% of 675 212 bytes is 33 760.6 — truncated, never rounded up.
        assert_eq!(Percent::from_hundredths(500).of(675_212), 33_760);
        assert_eq!(Percent::from_hundredths(500).of(0), 0);
    }

    #[test]
    fn threshold_accepts_sizes_and_percentages() {
        let absolute: Threshold = "50 KB".parse().unwrap();
        assert_eq!(absolute.allowed(1_000_000), 1_000_000 + 50 * 1024);
        assert_eq!(absolute.to_string(), "50 KB");

        let relative: Threshold = "5%".parse().unwrap();
        assert_eq!(relative.allowed(1_000_000), 1_050_000);
        assert_eq!(relative.to_string(), "5%");

        assert!("nope".parse::<Threshold>().is_err());
        assert!("-1 KB".parse::<Threshold>().is_err());
        assert!("-5%".parse::<Threshold>().is_err());
    }

    #[test]
    fn max_delta_accepts_percentages() {
        let baseline = SizeReport::new("app.wasm", 100_000, 40_000, 35_000);
        let max_delta = MaxDelta::raw_only("5%".parse().unwrap());

        // Exactly 5% is tolerated.
        let at_limit = SizeReport::new("app.wasm", 105_000, 40_000, 35_000);
        assert!(max_delta.exceeded_delta(&at_limit, &baseline).is_empty());

        let over = SizeReport::new("app.wasm", 105_001, 40_000, 35_000);
        assert_eq!(
            max_delta.exceeded_delta(&over, &baseline),
            vec![Metric::Raw]
        );

        // A percentage applies per metric, against that metric's baseline.
        let mut gzip = MaxDelta::new();
        gzip.set_limit(Metric::Gzip, Some("1%".parse().unwrap()));
        let gzip_over = SizeReport::new("app.wasm", 100_000, 40_500, 35_000);
        assert_eq!(
            gzip.exceeded_delta(&gzip_over, &baseline),
            vec![Metric::Gzip]
        );
        let raw_over_only = SizeReport::new("app.wasm", 200_000, 40_000, 35_000);
        assert!(gzip.exceeded_delta(&raw_over_only, &baseline).is_empty());
    }

    /// A percentage is meaningless as an absolute cap, so `budget` rejects it
    /// while `max_delta` accepts it.
    #[test]
    fn budget_rejects_percentages() {
        let dir = TempDir::new("budget_percent");
        let path = dir.path().join(CONFIG_FILE);
        std::fs::write(&path, r#"{"budget": {"raw": "5%"}}"#).unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(matches!(err, WasmCheckError::ConfigParse { .. }), "{err:?}");

        let path = dir.path().join("ok.json");
        std::fs::write(&path, r#"{"max_delta": {"raw": "5%"}}"#).unwrap();
        let config = Config::load(&path).unwrap();
        let threshold = config.max_delta().unwrap().limit(Metric::Raw).unwrap();
        assert_eq!(threshold.to_string(), "5%");
    }

    #[test]
    fn config_strict_defaults_to_false_and_round_trips() {
        let dir = TempDir::new("config_strict");
        let path = dir.path().join(CONFIG_FILE);

        let default = Config::new();
        assert!(!default.strict());
        default.save(&path).unwrap();
        assert!(!Config::load(&path).unwrap().strict());

        let mut config = Config::new();
        config.set_strict(true);
        config.save(&path).unwrap();
        assert!(Config::load(&path).unwrap().strict());
    }

    #[test]
    fn metric_round_trips_through_strings() {
        for metric in Metric::ALL {
            assert_eq!(metric.to_string().parse::<Metric>().unwrap(), metric);
            let json = serde_json::to_string(&metric).unwrap();
            assert_eq!(json, format!("\"{metric}\""));
            assert_eq!(serde_json::from_str::<Metric>(&json).unwrap(), metric);
        }
        assert!("heavy".parse::<Metric>().is_err());
    }

    // ---- Config -----------------------------------------------------------

    #[test]
    fn config_round_trips_unchanged() {
        let dir = TempDir::new("config_roundtrip");
        let path = dir.path().join(CONFIG_FILE);

        let mut config = Config::new();
        config.add_file("dist/assets/*_bg-*.wasm");
        let mut budget = Budget::new();
        budget.set_limit(Metric::Raw, Some("700 KB".parse().unwrap()));
        budget.set_limit(Metric::Gzip, Some("250 KB".parse().unwrap()));
        config.set_budget(Some(budget));

        config.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, config);

        let text = std::fs::read_to_string(&path).unwrap();
        // A budget keeps its human form across the round trip.
        assert!(text.contains("\"700 KB\""), "{text}");
        // Recorded sizes live in their own file, so the config stays small and
        // only changes when a human changes it.
        assert!(!text.contains("\"baseline\""), "{text}");
    }

    #[test]
    fn config_accepts_numeric_sizes() {
        let dir = TempDir::new("config_numeric");
        let path = dir.path().join(CONFIG_FILE);
        std::fs::write(&path, r#"{"budget":{"raw":10240}}"#).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(
            config.budget().unwrap().limit(Metric::Raw).unwrap().bytes(),
            10_240
        );
    }

    #[test]
    fn compression_defaults_to_the_cdn_preset() {
        assert_eq!(Compression::default(), Compression::cdn());
        assert!(Compression::default().is_default());
        assert!(!Compression::max().is_default());

        let dir = TempDir::new("compression_serde");
        let path = dir.path().join(CONFIG_FILE);

        // A config without a `compression` key keeps the CDN preset...
        std::fs::write(&path, r#"{"files":["app.wasm"]}"#).unwrap();
        assert_eq!(
            Config::load(&path).unwrap().compression(),
            Compression::cdn()
        );

        // ...and the preset is never written back, so configs stay minimal.
        Config::load(&path).unwrap().save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("compression"), "{text}");

        // A partial object defaults the missing field to the CDN preset.
        std::fs::write(&path, r#"{"compression":{"brotli_quality":11}}"#).unwrap();
        let partial = Config::load(&path).unwrap().compression();
        assert_eq!(partial.gzip_level(), 6);
        assert_eq!(partial.brotli_quality(), 11);

        // A full custom setting round-trips.
        std::fs::write(
            &path,
            r#"{"compression":{"gzip_level":9,"brotli_quality":11}}"#,
        )
        .unwrap();
        assert_eq!(
            Config::load(&path).unwrap().compression(),
            Compression::max()
        );

        // Out-of-range values are rejected when the config is read.
        std::fs::write(&path, r#"{"compression":{"gzip_level":10}}"#).unwrap();
        assert!(matches!(
            Config::load(&path).unwrap_err(),
            WasmCheckError::ConfigParse { .. }
        ));
    }

    #[test]
    fn compression_can_override_one_field_at_a_time() {
        let cdn = Compression::cdn();
        assert_eq!(cdn.with_gzip_level(1).unwrap().brotli_quality(), 5);
        assert_eq!(cdn.with_brotli_quality(11).unwrap().gzip_level(), 6);

        assert!(matches!(
            cdn.with_gzip_level(10).unwrap_err(),
            WasmCheckError::InvalidGzipLevel(10)
        ));
        assert!(matches!(
            cdn.with_brotli_quality(12).unwrap_err(),
            WasmCheckError::InvalidBrotliQuality(12)
        ));
        assert!(matches!(
            cdn.with_brotli_quality(-1).unwrap_err(),
            WasmCheckError::InvalidBrotliQuality(-1)
        ));

        // The default measures less aggressively than the maximum, so a budget
        // is not compared against a size that never reaches the wire.
        assert!(Compression::cdn().gzip_level() < Compression::max().gzip_level());
        assert!(Compression::cdn().brotli_quality() < Compression::max().brotli_quality());
        assert_eq!(
            Compression::max().to_string(),
            "gzip level 9, brotli quality 11"
        );
    }

    #[test]
    fn config_load_reports_missing_file() {
        let dir = TempDir::new("config_missing");
        let err = Config::load(dir.path().join("nope.json")).unwrap_err();
        assert!(matches!(err, WasmCheckError::ConfigNotFound(_)));
    }

    #[test]
    fn config_load_reports_bad_json() {
        let dir = TempDir::new("config_bad");
        let path = dir.path().join(CONFIG_FILE);
        std::fs::write(&path, "{ not json").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(matches!(err, WasmCheckError::ConfigParse { .. }));
    }

    #[test]
    fn baseline_is_keyed_by_the_glob() {
        let mut baseline = Baseline::new();
        baseline.set(
            "assets/*_bg-*.wasm",
            SizeReport::new("assets/app_bg-aaa.wasm", 100, 50, 40),
        );

        // The key is the glob, not the hashed filename it happened to match.
        assert_eq!(baseline.get("assets/*_bg-*.wasm").unwrap().raw(), 100);
        assert!(baseline.get("assets/app_bg-aaa.wasm").is_none());
    }

    #[test]
    fn baseline_prunes_stale_hashes() {
        let mut baseline = Baseline::new();
        baseline.set(
            "assets/*_bg-*.wasm",
            SizeReport::new("assets/app_bg-aaa.wasm", 100, 50, 40),
        );
        baseline.set(
            "assets/other_bg-bbb.wasm",
            SizeReport::new("assets/other_bg-bbb.wasm", 200, 90, 80),
        );
        baseline.set(
            "assets/keep.wasm",
            SizeReport::new("assets/keep.wasm", 5, 4, 3),
        );

        let keep = vec!["assets/*_bg-*.wasm".to_string()];
        baseline.prune("assets/*_bg-*.wasm", &keep);

        assert!(baseline.get("assets/*_bg-*.wasm").is_some());
        assert!(baseline.get("assets/other_bg-bbb.wasm").is_none());
        assert!(baseline.get("assets/keep.wasm").is_some());
    }

    #[test]
    fn baseline_is_a_plain_json_map() {
        let dir = TempDir::new("baseline_file");
        let path = dir.path().join(DEFAULT_BASELINE_FILE);

        let mut baseline = Baseline::new();
        baseline.set(
            "assets/*_bg-*.wasm",
            SizeReport::new("assets/app_bg-aaa.wasm", 100, 50, 40),
        );
        baseline.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with('{'), "{text}");
        assert!(text.contains("\"assets/*_bg-*.wasm\""), "{text}");

        let loaded = Baseline::load(&path).unwrap();
        assert_eq!(loaded, baseline);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.iter().count(), 1);
    }

    #[test]
    fn baseline_load_reports_a_missing_file() {
        let dir = TempDir::new("baseline_missing");
        let err = Baseline::load(dir.path().join("nope.json")).unwrap_err();
        assert!(
            matches!(err, WasmCheckError::BaselineNotFound(_)),
            "{err:?}"
        );
    }

    #[test]
    fn baseline_save_creates_missing_directories() {
        let dir = TempDir::new("baseline_dirs");
        let path = dir.path().join("ci/nested/baseline.json");
        Baseline::new().save(&path).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn baseline_path_follows_the_config() {
        let mut config = Config::new();
        assert!(
            config
                .baseline_path("dist")
                .ends_with(DEFAULT_BASELINE_FILE)
        );
        // `"."` (the default config directory) must not leak a `./` prefix.
        assert_eq!(
            config.baseline_path("."),
            std::path::PathBuf::from(DEFAULT_BASELINE_FILE)
        );

        config.set_baseline_file(Some("ci/base.json".into()));
        assert_eq!(
            config.baseline_path("dist"),
            std::path::PathBuf::from("dist/ci/base.json")
        );
        assert_eq!(config.baseline_file(), Some("ci/base.json"));
    }

    #[test]
    fn inline_baseline_is_readable_then_removed() {
        let dir = TempDir::new("config_inline_baseline");
        let path = dir.path().join(CONFIG_FILE);
        std::fs::write(
            &path,
            r#"{"files": ["app.wasm"],
                "baseline": {"app.wasm": {"file": "app.wasm", "raw": 10, "gzip": 5, "brotli": 4}}}"#,
        )
        .unwrap();

        let mut config = Config::load(&path).unwrap();
        assert_eq!(config.inline_baseline().len(), 1);

        let legacy = config.take_inline_baseline();
        assert_eq!(legacy["app.wasm"].raw(), 10);
        assert!(config.take_inline_baseline().is_empty());

        config.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"baseline\""), "{text}");
        assert!(Config::load(&path).unwrap().inline_baseline().is_empty());
    }

    // ---- Measurement ------------------------------------------------------

    #[test]
    fn reports_signed_deltas() {
        let before = SizeReport::new("app.wasm", 1000, 500, 400);
        let after = SizeReport::new("app.wasm", 1200, 480, 400);
        let delta = after.delta(&before);

        assert_eq!(delta.get(Metric::Raw), 200);
        assert_eq!(delta.get(Metric::Gzip), -20);
        assert_eq!(delta.get(Metric::Brotli), 0);
        assert_eq!(after.delta(&after).get(Metric::Raw), 0);

        let json = serde_json::to_value(delta).unwrap();
        assert_eq!(json["raw"], 200);
        assert_eq!(json["gzip"], -20);
    }

    #[test]
    fn measures_a_file_into_three_sizes() {
        let dir = TempDir::new("measure");
        let path = dir.path().join("test.wasm");
        let content = b"hello world, this is some test wasm content that repeats to make it compressible. hello world, this is some test wasm content that repeats to make it compressible.";
        std::fs::write(&path, content).unwrap();

        let report = SizeReport::measure(&path).unwrap();
        assert_eq!(report.raw(), content.len() as u64);
        assert!(report.gzip() > 0 && report.gzip() < report.raw());
        assert!(report.brotli() > 0 && report.brotli() < report.raw());
        assert_eq!(report.value(Metric::Raw), report.raw());
    }

    /// Regression: compression used to `unwrap()`, so a read/compression error
    /// panicked instead of being reported.
    #[test]
    fn measure_reports_errors_instead_of_panicking() {
        let dir = TempDir::new("measure_dir");
        let err = SizeReport::measure(dir.path()).unwrap_err();
        assert!(matches!(err, WasmCheckError::Io(_)), "got {err:?}");

        let missing = dir.path().join("nope.wasm");
        let err = SizeReport::measure(&missing).unwrap_err();
        assert!(
            matches!(err, WasmCheckError::FileNotFound(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn finds_wasm_files_only() {
        let dir = TempDir::new("find");
        std::fs::write(dir.path().join("test.wasm"), b"fake wasm").unwrap();
        std::fs::write(dir.path().join("other.txt"), b"not wasm").unwrap();

        let found = find_wasm_files(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("test.wasm"));
    }

    // ---- File resolution --------------------------------------------------

    #[test]
    fn config_entries_resolve_relative_to_the_config_dir() {
        let root = TempDir::new("resolve_relative");
        let crate_dir = root.path().join("dogfood-app");
        let assets = crate_dir.join("dist/assets");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("app_bg-abc.wasm"), b"x").unwrap();

        let mut config = Config::new();
        config.add_file("dist/assets/*_bg-*.wasm");

        let resolved = resolve_files(None, Some(&config), &crate_dir).unwrap();
        assert_eq!(resolved.len(), 1);
        // The key stays the glob so baselines survive a new content hash.
        assert_eq!(resolved[0].key(), "dist/assets/*_bg-*.wasm");
        assert!(resolved[0].path().is_file());
        assert!(resolved[0].path().starts_with(&crate_dir));
    }

    #[test]
    fn missing_config_entry_is_an_error() {
        let dir = TempDir::new("resolve_missing");
        let mut config = Config::new();
        config.add_file("dist/nope.wasm");
        let err = resolve_files(None, Some(&config), dir.path()).unwrap_err();
        assert!(matches!(err, WasmCheckError::FileNotFound(_)));
    }

    #[test]
    fn unmatched_glob_is_an_error() {
        let dir = TempDir::new("resolve_nomatch");
        let mut config = Config::new();
        config.add_file("nope-*.wasm");
        let err = resolve_files(None, Some(&config), dir.path()).unwrap_err();
        assert!(matches!(err, WasmCheckError::NoWasmFound { .. }));
    }

    #[test]
    fn cli_file_key_is_relative_to_the_config_dir() {
        let root = TempDir::new("resolve_cli");
        let crate_dir = root.path().join("app");
        let assets = crate_dir.join("dist");
        std::fs::create_dir_all(&assets).unwrap();
        let wasm = assets.join("app.wasm");
        std::fs::write(&wasm, b"x").unwrap();

        let resolved = resolve_files(Some(&wasm.to_string_lossy()), None, &crate_dir).unwrap();
        assert_eq!(resolved[0].key(), "dist/app.wasm");
    }

    // ---- top_functions ----------------------------------------------------

    #[test]
    fn top_functions_ranks_by_size() {
        let path = format!("{}/fixtures/full.wasm", env!("CARGO_MANIFEST_DIR"));
        let ranked = top_functions(&path, 3).unwrap();
        assert_eq!(ranked.len(), 3);
        assert!(ranked[0].0 >= ranked[1].0);
        assert!(ranked[1].0 >= ranked[2].0);

        let total = std::fs::metadata(&path).unwrap().len();
        assert!(ranked[0].0 <= total);
        assert!(!ranked[0].1.is_empty());
    }

    #[test]
    fn top_functions_truncates() {
        let path = format!("{}/fixtures/full.wasm", env!("CARGO_MANIFEST_DIR"));
        let ranked = top_functions(&path, 100).unwrap();
        assert!(!ranked.is_empty());
        assert!(ranked.len() <= 100);
    }

    /// A minimal module with **two imported functions** and one defined
    /// function named `defined` at function index 2 (imports occupy 0 and 1).
    fn wasm_with_imports(with_names: bool) -> Vec<u8> {
        let mut module = Vec::new();
        module.extend_from_slice(b"\0asm");
        module.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version 1

        // type section: one `() -> ()` type
        module.extend_from_slice(&[0x01, 0x04, 0x01, 0x60, 0x00, 0x00]);

        // import section: `env.a` and `env.b`, both functions of type 0
        module.extend_from_slice(&[0x02, 0x11, 0x02]);
        for name in *b"ab" {
            module.extend_from_slice(&[0x03]);
            module.extend_from_slice(b"env");
            module.extend_from_slice(&[0x01, name, 0x00, 0x00]);
        }

        // function section: one defined function of type 0
        module.extend_from_slice(&[0x03, 0x02, 0x01, 0x00]);

        // code section: one empty body
        module.extend_from_slice(&[0x0A, 0x04, 0x01, 0x02, 0x00, 0x0B]);

        if with_names {
            // name section: function names, entry index 2 -> "defined"
            module.extend_from_slice(&[0x00, 0x11, 0x04]);
            module.extend_from_slice(b"name");
            module.extend_from_slice(&[0x01, 0x0A, 0x01, 0x02, 0x07]);
            module.extend_from_slice(b"defined");
        }
        module
    }

    /// Regression: function indices in the name section count *imported*
    /// functions too. Numbering definitions from zero reported the wrong name
    /// for every wasm-bindgen artifact (all of which import).
    #[test]
    fn top_functions_accounts_for_imported_functions() {
        let dir = TempDir::new("top_imports");
        let path = dir.path().join("imports.wasm");
        std::fs::write(&path, wasm_with_imports(true)).unwrap();

        let ranked = top_functions(&path, 10).unwrap();
        assert_eq!(ranked.len(), 1, "only one function is defined: {ranked:?}");
        assert_eq!(ranked[0].1, "defined");
        assert!(ranked[0].0 > 0);
    }

    #[test]
    fn top_functions_falls_back_to_the_real_index() {
        let dir = TempDir::new("top_fallback");
        let path = dir.path().join("anon.wasm");
        // Same module without a name section, so only fallback names exist.
        std::fs::write(&path, wasm_with_imports(false)).unwrap();

        let ranked = top_functions(&path, 10).unwrap();
        assert_eq!(ranked.len(), 1);
        // The two imports occupy indices 0 and 1, so the definition is `func_2`.
        assert_eq!(ranked[0].1, "func_2");
    }
}
