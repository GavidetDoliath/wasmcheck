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

## Install

```sh
cargo install wasmcheck
```

## Quick start (2 minutes)

```sh
# 1. Create .wasmcheck.json with current sizes as the baseline
wasmcheck init --file target/wasm-unknown-unknown/release/app.wasm

# 2. Tune the budget (edit in .wasmcheck.json)
#    "budget": { "raw": "700 KB", "gzip": "250 KB" }

# 3. Gate your CI
wasmcheck check   # exit 0 = under budget, exit 1 = over budget
```

That's it. `check` reads `./.wasmcheck.json` automatically.

## CLI

```
wasmcheck [OPTIONS]                 # = wasmcheck check
wasmcheck check [--file] [--budget] [--config] [--format table|json] [--top N]
wasmcheck init [--file]             # create config from current sizes
wasmcheck baseline [--file]         # refresh baseline in existing config
```

| Flag | Meaning |
|------|---------|
| `--file <path>` | check a specific `.wasm` file (else auto-detect / config `files`) |
| `--budget "250 KB"` | raw size budget, overrides config |
| `--format json` | machine-readable output for CI/scripts |
| `--top 10` | list the heaviest functions (best-effort, needs name section) |

## Config `.wasmcheck.json`

```jsonc
{
  "files": ["dist/assets/*_bg-*.wasm"],   // globs supported
  "budget": {
    "raw": "700 KB",
    "gzip": "250 KB",                    // optional per-metric budgets
    "brotli": null
  },
  "baseline": {                          // auto-maintained, commit it
    "dist/assets/app_bg-abc123.wasm": {
      "file": "dist/assets/app_bg-abc123.wasm",
      "raw": 675212, "gzip": 240928, "brotli": 205110
    }
  }
}
```

Globs handle content-hashed filenames (`dx bundle`, `trunk build --release`)
that change on every build. Any configured budget that is exceeded fails the run.

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

`--format json` emits a single JSON object per file with `raw`, `gzip`, `brotli`
and optional `baseline`.

## `--top`: honest best-effort

Function sizes come from each function body's byte range; names come from the
wasm **name section**. Many release pipelines strip it (`dx bundle`, `wasm-opt`),
in which case functions are shown as `func_N`. It shines on raw
`target/wasm32-unknown-unknown/*/release/*.wasm` artifacts, which keep names.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | OK |
| 1 | budget exceeded, or error |

`Budget exceeded` failures always print `FAIL` and exit `1` — safe for CI.

## Developer notes

```
cargo test          # 30 tests (unit + integration)
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The `dogfood-app/` directory is a real Dioxus web app used to validate the tool
locally; run `cargo run --manifest-path Cargo.toml -- check --config dogfood-app/.wasmcheck.json`
after a `dx build --web --release` to replay the loop.

## License

MIT OR Apache-2.0