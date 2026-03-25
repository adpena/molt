# Kani Bounded Model Checking in CI

## Overview

[Kani](https://model-checking.github.io/kani/) is a bounded model checker for
Rust that uses CBMC under the hood.  It exhaustively explores all possible
inputs (within configurable bounds) to verify that assertions hold, providing
stronger guarantees than property-based testing alone.

Molt uses Kani to verify invariants of the NaN-boxed object model
(`molt-lang-obj-model`) and the runtime (`molt-runtime`).

## How CI works

The workflow lives in `.github/workflows/kani.yml` and runs on every push to
`main` and every pull request targeting `main`.

**Steps:**

1. Install the stable Rust toolchain.
2. Install the Kani verifier (`cargo install --locked kani-verifier && cargo kani setup`).
3. Run `cargo kani --tests` in `runtime/molt-obj-model` (NaN-boxing proofs).
4. Run `cargo kani --tests` in `runtime/molt-runtime` (runtime proofs).

The job has a 30-minute timeout.  If any harness fails, the entire job fails
and the check is reported on the PR.

## Expected runtime

The full harness suite typically completes in **5-10 minutes** on GitHub Actions
`ubuntu-latest` runners, depending on the number and complexity of harnesses.
Harnesses that explore large symbolic spaces (e.g., arbitrary `u64` bit patterns)
are the most expensive.

## How to add new harnesses

Harnesses live in test files gated behind `#[cfg(kani)]`:

```rust
// runtime/molt-obj-model/tests/kani_nanbox.rs

#[cfg(kani)]
mod kani_proofs {
    use molt_lang_obj_model::MoltObject;

    #[kani::proof]
    fn my_new_property() {
        let x: i64 = kani::any();
        kani::assume(x >= 0 && x <= 100);
        let obj = MoltObject::from_int(x);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(x));
    }
}
```

**Key patterns:**

- Use `kani::any()` to introduce a symbolic (nondeterministic) value.
- Use `kani::assume(condition)` to constrain the input space.
- Use standard `assert!` / `assert_eq!` for the property to verify.
- Wrap all harnesses in `#[cfg(kani)]` so they are invisible to `cargo test`.
- Each harness is annotated with `#[kani::proof]`.

**Adding a harness:**

1. Add a new `#[kani::proof]` function in the appropriate test file.
2. Run locally: `cd runtime/molt-obj-model && cargo kani --tests` (requires Kani installed).
3. Push -- CI will pick it up automatically.

## How to debug failures

### Local reproduction

```bash
# Install Kani (one-time)
cargo install --locked kani-verifier
cargo kani setup

# Run all harnesses in a crate
cd runtime/molt-obj-model
cargo kani --tests

# Run a single harness by name
cargo kani --tests --harness float_roundtrip
```

### Reading Kani output

When a harness fails, Kani prints a **counterexample trace** showing the exact
input values that violate the assertion.  Look for lines like:

```
VERIFICATION:- FAILED
  Counterexample:
    x = 42
```

The trace shows the concrete values assigned to each `kani::any()` call.

### Common issues

| Symptom | Likely cause |
|---------|-------------|
| Timeout (>30 min) | Harness explores too large a space. Add `kani::assume()` to narrow bounds or use `#[kani::unwind(N)]` to limit loop unrolling. |
| "unwinding assertion" | A loop needs a higher unwind bound. Add `#[kani::unwind(N)]` to the harness. |
| Spurious failure on `OnceLock` / `RwLock` | Kani has limited support for concurrency primitives. Avoid using the pointer registry in Kani harnesses; test pure NaN-boxing logic only. |
| `unsupported feature` | Some Rust / std features are not yet modeled by Kani. Check [Kani limitations](https://model-checking.github.io/kani/rust-feature-support.html). |

### Increasing verbosity

```bash
cargo kani --tests --harness my_harness --verbose
```

This prints the full CBMC solver output including variable assignments.
