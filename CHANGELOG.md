# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- `init` no longer seeds a budget that fails immediately: the raw budget is
  rounded **up** to the next KiB instead of to the nearest one.
- `--top` names now account for imported functions. Function indices in the
  name section include imports, so wasm-bindgen artifacts (which always import)
  previously reported names off by the import count.
- `baseline` and `init` honor the `files` list (including globs) from the config
  instead of ignoring it and auto-detecting in the working directory.
- Config `files` entries are resolved relative to the **config file's**
  directory, so `wasmcheck check --config dogfood-app/.wasmcheck.json` works
  from the repository root.
- Baselines are keyed by the matching `files` entry. A rebuild that changes a
  content hash (`app_bg-<newhash>.wasm`) keeps its delta instead of losing it.
- gzip/brotli failures are returned as errors instead of panicking in
  `.unwrap()`.
- Negative, non-finite and out-of-range sizes (`"-1 KB"`, `"nan"`) are rejected
  instead of saturating to `0` bytes.
- README: corrected the wasm target triple to
  `target/wasm32-unknown-unknown/release/app.wasm`.
- `rust-version` corrected from `1.85` to `1.88`. The declared minimum was a
  promise the crate could not keep: `wasmparser` 0.259 (used by `--top`)
  declares `rust-version = "1.88"`, so Cargo refuses to build the graph on
  anything older. A CI job now builds on exactly the declared minimum.

### Changed

- The recorded baseline now lives in **its own committed file** —
  `.wasmcheck.baseline.json` next to the config by default, or wherever the new
  `baseline_file` config key or `--baseline-file` flag points. `init` and
  `baseline` are the only writers, so a rebuild never touches `.wasmcheck.json`
  and pull requests stop conflicting on hundreds of lines of machine-generated
  numbers. An inline `"baseline"` block is still read for compatibility and is
  migrated into the file by the next `init` or `baseline`.
- `Baseline` is a public type with `load`, `save`, `get`, `set`, `remove` and
  `prune`; the baseline helpers left `Config`, which keeps only
  `inline_baseline`, `take_inline_baseline` and `clear_inline_baseline` for the
  deprecated inline copy, plus `baseline_path`.
- `Config::save` no longer writes `null` for unset `budget`, `max_delta` and
  `baseline_file`.
- gzip and brotli are now measured at the **CDN preset** (gzip level 6, brotli
  quality 5) instead of the maximum. The old numbers were smaller than what a
  server or CDN produces on the fly, so a budget could pass and still be blown
  in production. Set `compression` in the config (or the new CLI flags) to
  restore the maximum for precompressed assets. A committed gzip/brotli
  baseline must be refreshed after this change; the raw size is unaffected.
- `--format json` now emits a single JSON document for the whole run —
  `{"status": ..., "files": [...]}` — instead of one object per file, which
  produced invalid JSON as soon as two files were checked. Each file entry now
  also carries its signed `delta`, the `budget` that was applied, the
  `exceeded` metrics, and the `--top` ranking. stdout is always parseable;
  progress lines stay on stderr.
- Sizes are parsed into a validated `Size` type, so an invalid budget is
  rejected when the config is read rather than when a bundle is measured.
- `Budget` limits are typed and keyed by `Metric`; `Budget::limits_bytes` and
  `Budget::from_raw` are replaced by `Budget::limit`/`set_limit` and a
  `TryFrom<&str>` impl.
- Error messages are lowercase and free of trailing punctuation, per the
  Rust API guidelines.
- Public types derive the usual traits (`PartialEq`, `Eq`, `Copy`, `Hash`) and
  `Metric` implements `Display`/`FromStr` instead of an inherent `label()`.
- Added `SizeReport::delta` and the `SizeDelta` type for numeric, signed deltas
  (`delta_str` stays the table-formatted counterpart).

### Added

- Auto-detect falls back one level into `dist/` and `target/`: when no `.wasm`
  sits directly in the searched directory, `wasmcheck init` and `check` now
  find `dist/app.wasm` or `target/app.wasm` instead of failing with
  `no .wasm file found`. Deeper tooling layouts (`dist/assets/*.wasm`,
  `target/wasm32-unknown-unknown/release/*.wasm`) stay out of reach on
  purpose — point the config `files` list at a glob for those.
- `--file` accepts a **glob**, exactly like the config `files` list:
  `wasmcheck check --file "dist/*_bg-*.wasm"`. Every file a glob matches
  shares the glob as its baseline key, so `init --file "dist/*_bg-*.wasm"`
  records the glob and a rebuild under a new content hash keeps its delta.
- The `no .wasm file found` error now carries three tips: pass `--file` with
  the artifact path, use a glob in `--file`, or list paths and globs under
  `files` in `.wasmcheck.json`.
- A `max_delta` regression gate: `wasmcheck check --max-delta "50 KB"`, or the
  per-metric `max_delta` config key, fails the run when a bundle grows past the
  allowed amount over its committed baseline — even while well inside its
  absolute budget. Shrinking never fails, growth equal to the allowance is
  tolerated, and a file without a baseline entry is reported as not evaluated
  rather than silently ignored. JSON exposes `max_delta` and `delta_exceeded`.
- `max_delta` thresholds can be **percentages of the baseline** (`"5%"`, `"0.5%"`)
as well as absolute sizes, both in the config and on the command line. A
percentage resolves per metric against that metric's baseline; `budget` keeps
accepting sizes only, since a relative absolute-cap has no meaning.
- Strict mode (`--strict`, or `"strict": true`): a checked file with no baseline
entry fails the run instead of being quietly ungated. Off by default.
- `--format github` emits GitHub Actions workflow commands, so over-budget files
become annotations on the pull request that introduced them. Nothing is printed
when the run passes.
- New `Percent` and `Threshold` types; the limits type is now generic
(`Limits<Size>` = `Budget`, `Limits<Threshold>` = `MaxDelta`).
- `Budget::exceeded_delta`, the growth counterpart of `Budget::exceeded_metrics`.
- A `Compression` type and `compression` config key (`{"gzip_level": 9,
  "brotli_quality": 11}`), plus `--gzip-level` and `--brotli-quality` flags that
  override a single field. The default preset is omitted from saved configs,
  and the level used is reported in the JSON output.
- Crate-level documentation, `# Examples` and `# Errors` sections, and doctests.
- `LICENSE-MIT` and `LICENSE-APACHE` for the declared `MIT OR Apache-2.0`.
- crates.io metadata (`keywords`, `categories`, `readme`, `documentation`,
  `rust-version`) and an `include` list.
- README: a "Prior art" section comparing the existing JS size-budget gates
  ([size-limit](https://github.com/ai/size-limit), bundlesize, bundlewatch,
  compressed-size-action) and Rust profilers (twiggy, cargo-bloat), and what
  `wasmcheck` adds for wasm artifacts.

## [0.1.0] - 2026-09-13

### Added

- Initial release: `init`, `check` and `baseline` commands, raw/gzip/brotli
  measurement, per-metric budgets, committed baselines with deltas, glob support
  and `--top N`.
