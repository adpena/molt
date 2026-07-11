# Canonical Molt Product Surface

This document composes foundation 21d, 56, 57, 58, 59, 61, and 62 into one
product contract. It does not create a second compiler or deployment authority.

## 1. Product Invariant

The default experience is:

```text
molt run script.py
```

For the verified subset, stdout and exit status belong to the Python program.
Molt emits no successful-build chatter or raw tool output. A first run may show
one bounded progress line only when the budget is exceeded; repeat runs execute
from derived caches without asking the user to understand the compiler pipeline.

Budgets, measured from process start to first program output:

- warm verified-subset script: target **<= 250 ms**, strict release ceiling 1 s;
- prepared-project cold run with cached toolchain/runtime: target **<= 2 s**;
- unprepared machine: progress within 250 ms, actionable stage name, no silent
  stall, and an attested platform-specific ceiling until the build-speed arc
  reaches the 2 s target.

These are product budgets, distinct from the produced binary startup budgets
owned by foundation 62.

## 2. One Configuration Authority

Canonical project authority: `[tool.molt]` in `pyproject.toml`.

Python projects already require dependency and Python-version metadata there; a
second root file creates precedence and drift. Standalone scripts use PEP 723
metadata. `molt.toml` becomes a deleted legacy lane, not a peer source.

Precedence is closed and inspectable:

```text
explicit CLI override > pyproject/PEP 723 authority > derived default
```

Environment variables are internal transport or CI overrides, never primary
user configuration. Every accepted override appears in `molt config --resolved`
with value, source, and owning schema field. Unknown user-facing keys fail.
Malformed configuration fails at discovery; it is never silently ignored.

## 3. Composable Extension Algebra

The product model is a typed expression, not a bag of flags:

```text
Program(entry)
  + Packages(lock, extensions)
  + Target(native | wasm(host))
  + Profile(dev | release | named)
  + Capabilities(set)
  -> ArtifactSet
```

Closed operations:

- `Program + Packages` resolves one source/module/extension closure.
- Adding `Target` selects one ABI and host contract.
- Adding `Profile` selects optimization and artifact policy, not target identity.
- Adding `Capabilities` restricts runtime authority; it cannot change compilation
  semantics silently.
- The result is an explicit `ArtifactSet`: binary, single WASM module, or typed
  split-runtime bundle. Invalid compositions fail before compilation and name
  the incompatible operands.

Package custody remains upstream-owned. The algebra selects and composes package
build products; it never reimplements a package or bakes package semantics into
Molt.

## 4. Honest-Early Boundary

Every unsupported-subset failure must include:

1. stable error class: `MOLT_COMPAT_ERROR`;
2. precise feature and source location;
3. compatibility tier and impact;
4. canonical boundary document;
5. a specific replacement when known, otherwise the honest CPython/rewrite
   workaround;
6. no internal traceback unless `--verbose` is requested.

The gate is a mask-proof: compile a known unsupported construct and assert that
the boundary diagnostic appears before backend execution and that Python
traceback machinery does not.

## 5. Progressive Disclosure

- **Level 0 — Run:** `molt run app.py`. May expose program output, one bounded
  progress indicator, and product diagnostics only.
- **Level 1 — Build:** `molt build app.py [--target native|wasm]`. May expose
  artifact paths and the selected target/profile. Emit/link/sysroot/cache knobs
  are hidden behind named profiles or expert inspection.
- **Level 2 — Host:** `molt build app.py --target wasm:browser` or an equivalent
  typed target. The target derives split-runtime and loader topology; users do
  not combine boolean link/layout flags.
- **Level 3 — Extensions:** package metadata declares extensions; `molt build`
  resolves upstream build custody. Expert audit commands explain the derived
  plan without becoming required build steps.

## 6. Delete Or Hide

- Delete `molt.toml` as a peer project-config authority after one explicit
  migration release; do not preserve dual reads indefinitely.
- Hide `--emit`, linked/split-runtime booleans, sysroots, linker paths, cache
  paths, and backend implementation choices from ordinary help.
- Remove `--build-arg` from `run`; every supported run intent must be typed.
- Stop printing successful build paths during `run` unless `--verbose` or
  `--timing` is requested.
- Never expose raw compiler/linker warnings on a successful default run.
- Keep environment-only transport facts undocumented as product controls and
  reject conflicting user attempts through the config resolver.

## 7. First Landed Increments

This arc lands two teeth-bearing increments:

1. unsupported diagnostics always name the verified-subset boundary and a
   workaround, or a more specific replacement;
2. the generated Windows launcher uses `_dupenv_s` for the debug-env probe, so a
   default successful run no longer leaks the observed deprecation warning.

The next implementation arc should move config parsing to a schema-bearing
authority that rejects dual-source ambiguity, then replace `run --build-arg`
with typed target/profile/package composition.
