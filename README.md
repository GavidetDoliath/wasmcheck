# wasmcheck

A WASM bundle size budget checker for Rust + WebAssembly. Measures raw, gzip and
brotli sizes of your `.wasm` files, keeps a committed baseline with deltas, and
**fails your CI** when a budget is exceeded.

Built as a size-limit / bundlephobia-style gate, because the Rust/WASM world had
no budget tool — only profilers.

## Why

- `trunk`, `dx`, `wasm-pack` don't report or gate bundle size.
- `twiggy` tells you *why* code is big, but nothing says *stop shipping a bigger file*.
- gzip matters: a 675 KB wasm is only ~241 KB over the wire.

## Prior art

Size-budget CI gates exist in the JavaScript world, and profilers exist in the
Rust one — neither is shaped for a prebuilt `.wasm` artifact. If you only ship
JS, use [`size-limit`](https://github.com/ai/size-limit); it is the mature tool
for that.

| Tool | What it is | Gap vs `wasmcheck` |
|------|------------|--------------------|
| [`size-limit`](https://github.com/ai/size-limit) | The JS perf-budget gate: gzip/brotli budgets, fails the PR | Needs Node + `package.json`; its `file` plugin can check existing artifacts, but budgets stay absolute — no baseline, no growth gate |
| [`bundlesize`](https://github.com/siddharthkp/bundlesize) | Checks any file against a `maxSize` (gzip or brotli) | In maintenance mode; Node-based; no baseline, no growth gate |
| [`bundlewatch`](https://github.com/bundlewatch/bundlewatch) | Size tracking for any files | Node-based; its state lives in an external service, not a committed file |
| [`compressed-size-action`](https://github.com/preactjs/compressed-size-action) | GitHub Action reporting gzip/brotli on changed files | Report-and-comment; baseline is the PR base branch, no budget config, GitHub-Actions-only |
| [`twiggy`](https://github.com/AlexEne/twiggy) | wasm code-size profiler | Explains *why* code is big, gates nothing (`--top` is a mini-`twiggy` baked into the gate) |
| [`cargo-bloat`](https://github.com/RazrFalcon/cargo-bloat) | Binary size profiler for Rust (ELF/Mach-O/PE) | Profiler, not a gate — and it doesn't support WASM anyway |
| `wasm-pack` / `trunk` / `dx` | Build tooling | Measure nothing, gate nothing |

What `wasmcheck` adds on top: a single `cargo binstall`-able Rust binary pointed
at whatever `.wasm` your build already produced, with both an absolute budget
**and** a growth gate (`max_delta`) against a committed baseline — no Node in a
Rust CI.

## Install

```sh
cargo install wasmcheck        # once the crate is on crates.io
cargo install --git https://github.com/GavidetDoliath/wasmcheck   # until then
```

## Quick start (2 minutes)

```sh
# 1. Create .wasmcheck.json with current sizes as the baseline
wasmcheck init --file target/wasm32-unknown-unknown/release/app.wasm

# 2. Tune the budget (edit in .wasmcheck.json)
#    "budget": { "raw": "700 KB", "gzip": "250 KB" }

# 3. Gate your CI
wasmcheck check   # exit 0 = under budget, exit 1 = over budget
```

That's it. `check` reads `./.wasmcheck.json` automatically, plus the baseline it
points at (`.wasmcheck.baseline.json` next to it by default). Commit both.

`init` seeds the budget **above** the size it just measured, so a freshly
initialized config always passes its own first `check`.

## CLI

```
wasmcheck [OPTIONS]                 # = wasmcheck check
wasmcheck check [--file] [--budget] [--max-delta] [--strict] [--baseline-file] [--config] [--gzip-level N] [--brotli-quality N] [--format table|json|github] [--top N]
wasmcheck init [--file] [--config]  # create config from current sizes
wasmcheck baseline [--file] [--config]   # refresh baseline in existing config
```

| Flag | Meaning |
|------|---------|
| `--file <path>` | check a specific `.wasm` file — a **glob** works too, like the config `files` list (else config `files`, else auto-detect) |
| `--budget "250 KB"` | raw size budget, overrides the config's raw limit |
| `--max-delta "50 KB"` | fail when a file grows more than SIZE over its baseline, even under budget (raw size; `"5%"` works too) |
| `--strict` | fail when a checked file has no baseline entry |
| `--baseline-file <path>` | baseline file to read or write (default: the config's `baseline_file`, else next to the config) |
| `--config <path>` | config file to use (default `./.wasmcheck.json`) |
| `--gzip-level <0-9>` | gzip level to measure with (default `6`, what a CDN does on the fly) |
| `--brotli-quality <0-11>` | brotli quality to measure with (default `5`) |
| `--format json` | machine-readable output for CI/scripts |
| `--format github` | GitHub Actions annotations for the files that fail |
| `--top 10` | list the heaviest functions (best-effort, needs name section) |

`init` and `baseline` use the `files` list from `--config` when one is given, so
`wasmcheck baseline --config dogfood-app/.wasmcheck.json` refreshes the hashed
artifacts that config already points at.

## Config `.wasmcheck.json`

```jsonc
{
  // Paths and globs, resolved relative to THIS config file's directory.
  "files": ["dist/assets/*_bg-*.wasm"],
  "budget": {
    "raw": "700 KB",
    "gzip": "250 KB",                    // optional per-metric budgets
    "brotli": null
  },
  "max_delta": {                        // optional growth allowance vs baseline
    "raw": "50 KB",
    "gzip": "2%",                       // percentages are allowed here
    "brotli": null
  },
  "strict": false,                      // fail on files with no baseline entry
  // Optional: how hard to compress before measuring. Default is the CDN
  // preset (gzip 6, brotli 5) and is left out of the file.
  "compression": { "gzip_level": 9, "brotli_quality": 11 },
  // Optional: where the baseline lives, relative to this config.
  // Default: ".wasmcheck.baseline.json" next to this file.
  "baseline_file": "ci/baseline.json"
}
```

Globs handle content-hashed filenames (`dx bundle`, `trunk build --release`)
that change on every build. Any configured budget that is exceeded fails the run.

## Compression: what the numbers model

The gzip and brotli numbers are **not** the smallest your bundle could possibly
be — they are the size that goes over the wire when a server or CDN compresses
on the fly:

| Metric | Default | [`Compression::max()`] |
|--------|---------|------------------------|
| gzip   | level 6 | level 9                |
| brotli | quality 5 | quality 11           |

Measuring at the maximum would report a size smaller than what most hosts
actually send, so a budget could pass here and still be blown in production.
Use `Compression::max()` when your assets are precompressed at build time and
served as-is:

```jsonc
"compression": { "gzip_level": 9, "brotli_quality": 11 }
```

```sh
wasmcheck check --gzip-level 9 --brotli-quality 11
```

A CLI flag overrides only the field it names, so `--gzip-level 9` keeps the
configured brotli quality. Gzip and brotli are lossless codecs, so the setting
never changes the raw size; changing it does invalidate a committed gzip/brotli
baseline, though — refresh it with `wasmcheck baseline`.

[`Compression::max()`]: https://docs.rs/wasmcheck/latest/wasmcheck/struct.Compression.html#method.max

## Baseline `.wasmcheck.baseline.json`

The recorded sizes live in **their own committed file**, next to the config by
default. `init` and `baseline` are the only things that write it, so a rebuild
never touches `.wasmcheck.json` — the file humans edit — and pull requests stop
conflicting on hundreds of lines of machine-generated numbers.

```json
{
  "dist/assets/*_bg-*.wasm": {
    "file": "dist/assets/app_bg-abc123.wasm",
    "raw": 675212,
    "gzip": 240928,
    "brotli": 205110
  }
}
```

It is keyed by the matching `files` entry, not by the hashed filename, so a new
content hash keeps its delta. Point it elsewhere with `baseline_file` in the
config, or per run with `--baseline-file` — useful when the baseline comes from
a CI artifact rather than the checkout:

```sh
wasmcheck check --baseline-file /tmp/baseline-from-main.json
```

Configs written before this split still work: an inline `"baseline"` block is
read as before and moved into the file by the next `init` or `baseline`.

Because resolution is relative to the config file, a config committed next to a
sub-crate works from anywhere:

```sh
wasmcheck check --config dogfood-app/.wasmcheck.json   # run from the repo root
```

## Regression gate: `max_delta`

An absolute budget tells you when a bundle is *too big*; `max_delta` tells you
when it *grew too fast*. A file can be comfortably inside its budget and still
fail:

```jsonc
"budget":    { "raw": "700 KB" },   // 675 KB is fine …
"max_delta": { "raw": "50 KB" }     // …but +60 KB since the baseline is not
```

```sh
wasmcheck check --max-delta "50 KB"   # CLI equivalent, applies to raw size
```

A threshold is either an absolute size or a **percentage of the baseline**, one
value per metric:

```jsonc
"max_delta": { "raw": "5%", "gzip": "2%" }
```

```sh
wasmcheck check --max-delta "5%"
```

- Growth is measured per metric against the committed baseline, and a
percentage resolves against that same metric's baseline.
- Growth exactly equal to the allowance is tolerated; shrinking never fails;
percentages truncate to whole bytes.
- A percentage is meaningless as an absolute cap, so `budget` only accepts sizes.
- A file with no baseline entry cannot be compared, so it is not failed — the
table output prints `no baseline recorded for this file` so the gap is visible
instead of silent. Use strict mode to make that gap a failure.

## Strict mode

`max_delta` can only gate files that *have* a baseline, which makes a fresh
clone or a newly added asset silently ungated. `--strict` (or `"strict": true`
in the config) turns that into a failure:

```sh
wasmcheck check --strict
```

```
FAIL dist/assets/app_bg-new.wasm: no baseline recorded
```

It is off by default, so a first run without a committed baseline still reports
sizes instead of failing.

## CI

```yaml
# .github/workflows/wasm-size.yml
name: wasm-size
on: [push, pull_request]
jobs:
  size:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { targets: wasm32-unknown-unknown }
      - name: Build wasm
        run: cargo build --release --target wasm32-unknown-unknown
      - uses: taiki-e/install-action@cargo-binstall
      - name: Install wasmcheck
        run: cargo binstall -y wasmcheck
      - name: Check bundle budgets
        run: wasmcheck check
```

## Output

```
  file: dist/assets/app_bg-abc123.wasm
  raw       675.21 KB  -1 B
  gzip      240.92 KB  +5.64 KB  over budget
  brotli    198.18 KB  -2.12 KB

PASS dist/assets/app_bg-abc123.wasm   # or: FAIL ...  exceeds budget: gzip
```

### JSON output

`--format json` writes **one JSON document for the whole run** on stdout, so it
stays parseable whatever the number of files, and so the PASS/FAIL verdict and
the `--top` ranking travel with the numbers. Progress and error lines go to
stderr, leaving stdout machine-readable even when a file fails.

```json
{
  "status": "fail",
  "compression": { "gzip_level": 6, "brotli_quality": 5 },
  "files": [
    {
      "file": "dist/assets/app_bg-abc123.wasm",
      "raw": 675212,
      "gzip": 240928,
      "brotli": 205110,
      "baseline": { "file": "dist/assets/app_bg-abc123.wasm", "raw": 675212, "gzip": 235000, "brotli": 207000 },
      "delta": { "raw": 0, "gzip": 5928, "brotli": -1890 },
      "budget": { "raw": "700 KB", "gzip": "250 KB", "brotli": null },
      "max_delta": { "raw": "50 KB", "gzip": null, "brotli": null },
      "exceeded": ["gzip"],
      "delta_exceeded": ["raw"],
      "status": "fail",
      "top": [{ "size": 9123, "name": "dioxus_core::diff::node::VNode::diff_node" }]
    }
  ]
}
```

| Field | Meaning |
|-------|---------|
| `status` | `"pass"`, `"fail"`, or `"unchecked"` when no budget is configured |
| `compression` | the gzip level and brotli quality the numbers were measured with |
| `files[].raw/gzip/brotli` | measured sizes, in bytes |
| `files[].baseline` | the committed baseline, omitted when there is none |
| `files[].delta` | signed byte difference vs the baseline, omitted when there is none |
| `files[].budget` | the absolute limits that were applied, in config form |
| `files[].baseline_missing` | `true` when strict mode flagged a missing baseline |
| `files[].max_delta` | the growth allowances that were applied, in config form |
| `files[].exceeded` | metrics over their budget, omitted when empty |
| `files[].delta_exceeded` | metrics that grew past `max_delta`, omitted when empty |
| `files[].top` | `--top` ranking as `{size, name}`, omitted without `--top` |

The exit code is unchanged: `1` as soon as one file fails — over budget or past
its `max_delta` — even in JSON mode.

### GitHub annotations

`--format github` emits [workflow commands](https://docs.github.com/actions/reference/workflow-commands-for-github-actions)
on stdout, so failures land as annotations on the pull request that introduced
them instead of only in the log:

```
::error file=dist/assets/app_bg-abc123.wasm,title=wasmcheck::gzip over budget: 265.02 KB > 250.00 KB
::error file=dist/assets/app_bg-abc123.wasm,title=wasmcheck::raw grew past max delta: +60.00 KB > 50 KB
```

A passing run prints nothing. `--top` is not part of this format — annotations
are for problems, not for reports. Paths are emitted as `wasmcheck` saw them, so
run it from the repository root for the links to resolve.

## `--top`: honest best-effort

Function sizes come from each function body's byte range; names come from the
wasm **name section**. Function index spaces include imported functions, and
`wasmcheck` accounts for them, so names line up with the real definitions.

Many release pipelines still strip the name section (`dx bundle`, `wasm-opt`),
in which case functions are shown as `func_N`. It shines on raw
`target/wasm32-unknown-unknown/release/*.wasm` artifacts, which keep names.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | OK |
| 1 | a gate failed (budget or `max_delta`), or an error |

Gate failures always print `FAIL` and exit `1` — safe for CI. A run with no
`budget` and no `max_delta` gates nothing: it reports sizes and exits `0`
(`"status": "unchecked"` in JSON).

## Developer notes

```
cargo test          # unit + integration + doc tests
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo publish --dry-run
```

The `dogfood-app/` directory is a real Dioxus web app used to validate the tool
locally; run `cargo run -- check --config dogfood-app/.wasmcheck.json` after a
`dx build --web --release` to replay the loop.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
