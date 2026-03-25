# Molt Fuzz Testing

Rust-level fuzz testing infrastructure using [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz)
and libFuzzer. These targets exercise critical Rust code paths that process
untrusted or arbitrary input.

## Prerequisites

```bash
cargo install cargo-fuzz
rustup install nightly
```

## Available Targets

| Target              | What it tests                                           |
|---------------------|---------------------------------------------------------|
| `fuzz_ir_parse`     | JSON deserialization of SimpleIR (`from_json_str`)      |
| `fuzz_wasm_compile` | Full WASM compilation pipeline from arbitrary IR        |
| `fuzz_nan_boxing`   | NaN-boxing encode/decode roundtrip invariants           |
| `fuzz_ir_passes`    | Optimization passes (fold, inline, escape, RC coalesce) |
| `fuzz_ir_validate`  | IR validation with edge-case field combinations         |

## Running

From the **repository root**:

```bash
# Run a single target (5 minutes, 4KB max input):
cargo +nightly fuzz run fuzz_nan_boxing -- -max_len=4096 -max_total_time=300

# Run with more workers:
cargo +nightly fuzz run fuzz_wasm_compile -- -max_len=4096 -jobs=4 -workers=4

# Run all targets sequentially:
for target in fuzz_ir_parse fuzz_wasm_compile fuzz_nan_boxing fuzz_ir_passes fuzz_ir_validate; do
  echo "=== $target ==="
  cargo +nightly fuzz run "$target" -- -max_len=4096 -max_total_time=60
done

# List available targets:
cargo +nightly fuzz list
```

## Reproducing Crashes

When a crash is found, libFuzzer saves the input to `fuzz/artifacts/<target>/`.
Reproduce it with:

```bash
cargo +nightly fuzz run fuzz_nan_boxing fuzz/artifacts/fuzz_nan_boxing/crash-<hash>
```

## Corpus Management

Corpus files accumulate in `fuzz/corpus/<target>/`. To minimize:

```bash
cargo +nightly fuzz cmin fuzz_nan_boxing
```

## Coverage

Generate a coverage report to see which code paths the fuzzer has explored:

```bash
cargo +nightly fuzz coverage fuzz_nan_boxing
```

## Adding New Targets

1. Create `fuzz/fuzz_targets/fuzz_<name>.rs` with `#![no_main]` and `fuzz_target!`
2. Add a `[[bin]]` entry in `fuzz/Cargo.toml`
3. Use `#[derive(Arbitrary)]` for structured input generation where possible
4. Focus on invariants: roundtrip properties, no-panic guarantees, consistency checks
