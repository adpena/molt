# Molt Harness Engineering: Unified Quality Enforcement

**Status:** Draft — Pending Review
**Date:** 2026-03-28
**Scope:** Local-first testing, benchmarking, fuzzing, and validation infrastructure
**Prerequisite:** Phase 0 of Monty Integration (resource controls, audit, DoS guards)

---

## 1. Goal

Build a purpose-built `molt harness` command that unifies all quality enforcement
behind a single local-first CLI with layered profiles. Every correctness, performance,
security, and optimization requirement is a hard gate with zero tolerance. The harness
replaces the current fragmented testing approach (scattered cargo test, pytest, molt
test, molt diff, molt compare) with one command that answers: "Is this code ready?"

## 2. Non-Goals

- Replacing CI. CI runs the same `molt harness` command; it is not a separate system.
- Runtime monitoring. The harness validates code at dev/build time, not in production.
- Visual regression testing. Molt does not produce visual output.

## 3. Design Principles

1. **Local-first.** Every gate runs on the developer's machine. CI is a backstop,
   not the primary enforcement point.
2. **Zero tolerance.** Every gate is hard. No soft warnings. No ignored tests.
   Any regression blocks until investigated.
3. **Ratchet-only.** Quality metrics only move forward. Achieving a higher mark
   automatically raises the floor. Lowering a baseline requires explicit
   `--lower-baseline` with a logged justification.
4. **Fast feedback.** The `quick` profile completes in <30 seconds. Developers
   run it before every commit. Depth is additive, not mandatory.
5. **Evidence-based.** Every claim of "passing" requires fresh verification
   output. No caching of pass/fail status across runs.

---

## 4. Architecture

### 4.1 Components

**Python orchestrator** (`src/molt/harness.py`)
- Profile selection and layer sequencing
- Subprocess dispatch (cargo, pytest, node, fuzz)
- Result collection into structured `HarnessReport`
- Console, JSON, and HTML report generation
- Baseline management (save, compare, ratchet)

**Rust test library** (`runtime/molt-harness/`)
- Resource enforcement verification (WASM modules with limits)
- Audit event schema validation
- Determinism checks (native vs WASM output comparison)
- Binary/WASM size measurement
- Criterion benchmarks (CodSpeed-compatible via `codspeed-criterion-compat`)
- Deterministic benchmark dataset generation (fixed RNG seed)

**Test corpus** (`tests/harness/`)
- Python test files for differential/conformance testing
- Jinja2 templates for generated combinatorial tests
- Baselines (committed), expectations (hashed), reports (gitignored)

### 4.2 CLI Interface

```
molt harness [profile]                — run a profile (default: standard)
molt harness quick                    — compile + lint + unit (~30s)
molt harness standard                 — + wasm + differential + resource + audit (~5m)
molt harness deep                     — + fuzz + conformance + bench + size + mutation (~30m)
molt harness report                   — print trend table from last N runs
molt harness report --html            — self-contained HTML report with diffs
molt harness report --json            — machine-readable report
molt harness complete-tests           — auto-fill expectations from CPython
molt harness bench --save-baseline X  — save current perf as named baseline
molt harness bench --compare X        — compare against named baseline
molt harness fuzz-all [duration]      — parallel fuzz all targets (default: 10m)
molt harness generate-tests           — expand Jinja2 templates into test corpus
```

### 4.3 Profile Model

Each profile is a strict superset of the previous. Running `molt harness` with
no argument defaults to `standard`.

```
quick    = compile + lint + unit-rust + unit-python
standard = quick + wasm-compile + differential + resource + audit
deep     = standard + fuzz + conformance + bench + size + mutation + determinism + miri + compile-fail
```

---

## 5. Layer Definitions

### 5.1 Layer Execution Order

Layers run in dependency order. If a layer fails, subsequent layers are skipped
unless `--no-fail-fast` is passed (which runs all layers and reports all failures).

| # | Layer | Profile | Command | Duration |
|---|-------|---------|---------|----------|
| 1 | `compile` | quick | `cargo check` all workspace crates | ~5s |
| 2 | `lint` | quick | `cargo clippy -- -D warnings` on molt-authored crates | ~3s |
| 3 | `unit-rust` | quick | `cargo nextest run` all crates, 3 feature-flag modes | ~15s |
| 4 | `unit-python` | quick | `python3 -m molt.capability_manifest` + pytest harness tests | ~5s |
| 5 | `wasm-compile` | standard | `molt build --target wasm` on test corpus | ~30s |
| 6 | `differential` | standard | `molt diff` on corpus, 3 backend modes | ~120s |
| 7 | `resource` | standard | WASM modules with limits — all enforcement scenarios | ~30s |
| 8 | `audit` | standard | Capability-gated ops → validate emitted JSON events | ~10s |
| 9 | `fuzz` | deep | All fuzz targets in parallel, configurable duration | ~600s |
| 10 | `conformance` | deep | Monty's 250 test files through Molt differential | ~120s |
| 11 | `bench` | deep | Criterion benchmarks, compare to saved baseline | ~60s |
| 12 | `size` | deep | Binary + WASM size for reference programs | ~10s |
| 13 | `mutation` | deep | `cargo mutants` on security-critical modules | ~300s |
| 14 | `determinism` | deep | Native vs WASM output comparison (Nuitka pattern) | ~60s |
| 15 | `miri` | deep | `cargo miri test` on unsafe boundary code | ~120s |
| 16 | `compile-fail` | deep | `trybuild` compile-fail test cases | ~15s |

### 5.2 Feature-Flag Matrix (Layer 3)

Inspired by Monty's triple feature-flag testing. The `unit-rust` layer runs tests
three ways to catch different bug classes:

1. **Default features** — normal execution
2. **`--features refcount_verify`** — panics on refcount leaks or double-free
3. **`--features audit`** — verifies audit events are emitted on capability checks

All three must pass with 0 failures.

### 5.3 Backend Matrix (Layer 6)

Inspired by Buffa's three-mode conformance. The `differential` layer runs parity
tests three ways:

1. **Native backend** (Cranelift) — `molt run`
2. **WASM backend** (unlinked) — `molt build --target wasm` + WASM host
3. **WASM backend** (linked) — `molt build --target wasm --linked` + Node.js

All three must produce identical output for every non-xfail test case.

### 5.4 Resource Enforcement Scenarios (Layer 7)

Six mandatory scenario categories:

1. **Time limit** — infinite loop with `max_duration=1s` must terminate within 2s
2. **Memory limit** — allocation loop with `max_memory=1MB` must raise uncatchable error
3. **Allocation limit** — rapid alloc with `max_allocations=1000` must reject
4. **Recursion limit** — deep recursion with `max_recursion_depth=50` must raise RecursionError
5. **DoS guard: pow** — `2 ** 10_000_000` with default limits must raise MemoryError
6. **DoS guard: repeat** — `'x' * 10_000_000_000` with default limits must raise MemoryError

Each scenario also verifies:
- The error is uncatchable by Python `try/except` (except RecursionError)
- An audit event is emitted for the violation
- The WASM module terminates cleanly (no hang, no crash)

### 5.5 Fuzz Targets (Layer 9)

Inspired by Buffa's parallel fuzz-all runner. All targets run simultaneously:

| Target | Input | Property |
|--------|-------|----------|
| `fuzz_nan_boxing` | Random `u64` | Encode/decode roundtrip, tag exclusivity |
| `fuzz_wasm_type_section` | Random WASM types | Encode → validate → parse roundtrip |
| `fuzz_tir_passes` | Random TIR graphs | No panics through 8-pass pipeline |

Default duration: 10 minutes per target. `deep` profile. Progress summary every
60 seconds. Any crash is a hard failure.

### 5.6 Combinatorial Test Generation (Monty + Nuitka pattern)

Jinja2 templates generate test files crossing types with operations:

```jinja2
{# template: type_x_operator.py.j2 #}
{% for type in ["int", "float", "bool", "str", "list", "tuple"] %}
{% for op in ["+", "-", "*", "**", "//", "%", "<<", ">>", "&", "|", "^"] %}
def test_{{ type }}_{{ op | replace('*', 'mul') | replace('+', 'add') }}():
    ...
{% endfor %}
{% endfor %}
```

`molt harness generate-tests` expands templates into `tests/harness/corpus/generated/`.
Generated files are committed (not gitignored) so they're available without Jinja2
installed.

---

## 6. Acceptance Criteria (All Hard Gates)

### 6.1 Zero-Tolerance Gates

| Criterion | Threshold | Measurement |
|-----------|-----------|-------------|
| Compilation | 0 errors, 0 warnings on molt-authored crates (molt-runtime, molt-backend, molt-snapshot, molt-embed, molt-harness) | `cargo check` + `cargo clippy -D warnings` |
| Rust unit tests | 0 failures, 0 ignored | `cargo nextest` (no `#[ignore]` permitted) |
| Python unit tests | 0 failures | pytest + inline test runners |
| Test count floor | >= baseline count per crate | Compared to `baselines/baseline.json` |
| WASM compilation | 100% of corpus compiles | `molt build --target wasm` |
| CPython parity | 0 divergences on non-xfail tests | `molt diff` |
| Strict xfail | Passing xfail = hard error | Xfail tests that succeed block the run |
| Resource enforcement | 100% of scenarios pass | All 6 categories, all sub-checks |
| Audit schema | 0 violations, 0 missing events | JSON schema validation |
| Fuzz crashes | 0 crashes | All targets, full duration |
| Feature-flag matrix | All 3 modes pass | default + refcount_verify + audit |
| Backend matrix | All 3 backends identical output | native + wasm + wasm-linked |
| Miri | 0 undefined behavior | `cargo miri test` on boundary code |
| Compile-fail | All expected rejections verified | `trybuild` |

### 6.2 Ratcheting Gates

| Criterion | Initial Floor | Ratchet Rule |
|-----------|--------------|--------------|
| Monty conformance pass rate | 0% (no tests yet) | Automatically raised to highest achieved |
| Mutation score on security modules (`resource.rs`, `audit.rs`, `caps.rs`, `ops_arith.rs` guard code) | 0% (not yet run) | Automatically raised to highest achieved |
| Performance (any tracked metric) | First `deep` run baseline | Any regression requires investigation |
| Binary size (native + WASM) | First `deep` run baseline | Any growth requires investigation |
| Determinism (native vs WASM) | Not yet measured | Once achieved, must hold |

### 6.3 Policy Rules

- **No `#[ignore]`** on any test. If a test cannot pass, fix it or delete it
  with a linked tracking issue.
- **No `xfail` without an issue link.** Every xfail must reference why and when
  it will be resolved.
- **No `#[allow(clippy::*)]`** without a comment explaining why the lint is
  wrong, not why it is inconvenient.
- **Performance investigation is mandatory** on any detected change, even 0.1%.
  The question is "why did this change?" not "is this change acceptable?"
- **Lowering a baseline** requires `--lower-baseline "justification string"`
  which is logged to the report history.

---

## 7. File Layout

```
src/molt/
├── harness.py                — orchestrator: profiles, layer sequencing, dispatch
├── harness_layers.py         — individual layer implementations
└── harness_report.py         — console table, JSON, HTML report generation

runtime/molt-harness/
├── Cargo.toml
├── src/
│   ├── lib.rs                — public API
│   ├── resource_enforcement.rs
│   ├── audit_verification.rs
│   ├── determinism.rs
│   └── size_tracking.rs
├── benches/
│   ├── tracked.rs            — criterion benchmarks (CodSpeed-compatible)
│   └── datasets.rs           — deterministic input generation
└── tests/
    ├── resource_scenarios.rs
    └── audit_scenarios.rs

tests/harness/
├── corpus/
│   ├── basic/                — core language test files
│   ├── stdlib/               — stdlib coverage
│   ├── resource/             — resource enforcement scenarios
│   ├── audit/                — audit event scenarios
│   ├── generated/            — Jinja2-expanded combinatorial tests (committed)
│   └── monty_compat/         — Monty's 250 test files
├── templates/                — Jinja2 templates for test generation
├── baselines/
│   └── baseline.json         — tracked metrics (committed)
├── expectations/
│   └── hashes.txt            — hashed expected outputs (committed)
└── reports/                  — run reports (gitignored)
```

---

## 8. Patterns Adopted from Prior Art

| Pattern | Source | Application |
|---------|--------|-------------|
| Strict xfail (passing = error) | Monty | `differential` layer |
| Auto-completion of expectations | Monty | `molt harness complete-tests` |
| Triple feature-flag testing | Monty | `unit-rust` layer (3 modes) |
| Three-mode conformance | Buffa | `differential` layer (3 backends) |
| Parallel fuzz-all runner | Buffa | `fuzz` layer |
| CodSpeed + Criterion dual-mode | Monty | `bench` layer |
| Cross-impl Docker benchmarks | Buffa | `bench` layer: Molt vs CPython on tracked workloads (Docker-isolated) |
| Deterministic benchmark datasets | Buffa | `bench` layer (fixed RNG) |
| Hashed reference system | Typst | `expectations/hashes.txt` |
| Self-contained HTML report | Typst | `molt harness report --html` |
| Self-compilation determinism | Nuitka | `determinism` layer |
| Generated/templated tests | Nuitka | `molt harness generate-tests` |
| Sequential CI dependencies | Nuitka | Linux gates macOS/WASM jobs |
| Miri for unsafe verification | Monty + Typst | `miri` layer |
| Compile-fail API safety | Monty | `compile-fail` layer |
| Mutation testing on security code | Novel | `mutation` layer on resource/audit/caps |

---

## 9. Testing Strategy for the Harness Itself

The harness must be tested. Layers are unit-testable because each is a pure
function: `fn run_layer(config) -> LayerResult`.

- `test_compile_layer_detects_error` — inject a syntax error, verify layer fails
- `test_ratchet_raises_floor` — verify baseline.json updates on improvement
- `test_ratchet_blocks_regression` — verify lower score fails without flag
- `test_strict_xfail_catches_stale` — verify passing xfail blocks
- `test_report_json_schema` — verify report output matches expected schema
- `test_profile_superset` — verify `standard` includes all `quick` layers
- `test_feature_flag_matrix` — verify all 3 modes are actually invoked

---

## 10. Integration with Existing `molt` Commands

The harness subsumes and unifies existing commands:

| Existing Command | Harness Equivalent |
|------------------|--------------------|
| `cargo test -p molt-runtime` | `molt harness quick` (layer: unit-rust) |
| `molt diff` | `molt harness standard` (layer: differential) |
| `molt compare` | `molt harness deep` (layer: bench) |
| `molt test` | `molt harness standard` (layers: unit + wasm + differential) |
| `cargo fuzz run` | `molt harness fuzz-all` or `molt harness deep` (layer: fuzz) |

The existing commands continue to work unchanged. The harness orchestrates them
rather than replacing them. This avoids migration friction while providing a
single entry point for comprehensive validation.
