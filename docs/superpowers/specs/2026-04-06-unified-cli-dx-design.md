# Unified CLI DX Design

**Status:** Approved
**Date:** 2026-04-06
**Scope:** Canonical developer and user command surface for setup, diagnosis,
validation, and thin-wrapper install flows
**Primary goal:** Make Molt feel like one coherent cross-platform tool instead
of a repo with multiple overlapping bootstrap and validation entrypoints

---

## 1. Goal

Collapse Molt's setup, diagnosis, installation, contributor workflows, and
release-readiness verification into one canonical CLI authority:

- `molt setup`
- `molt doctor`
- `molt validate`

Everything else must delegate into that surface instead of owning parallel
behavior.

The design target is closer to `go` than to a typical script pile:

- one primary tool;
- one obvious path;
- first-party validation;
- explicit environment introspection;
- strong defaults with capability-aware hard failures.

## 2. Non-Goals

- Preserving duplicated behavior across `install.sh`, `install.ps1`,
  `tools/dev.py`, and `molt`.
- Adding silent degradation when toolchains or targets are missing.
- Replacing the existing backend/conformance architecture with a new harness.
- Hiding optional-toolchain gaps behind best-effort behavior.
- Introducing a second config authority outside the canonical CLI.

## 3. Problem Statement

The current DX shape has four structural weaknesses:

1. **Too many behavioral authorities.**
   `packaging/install.sh`, `packaging/install.ps1`, `tools/dev.py`, and
   `src/molt/cli.py` each own part of setup/update/validation behavior.

2. **Incomplete end-to-end authority.**
   There is not yet a single command that clearly owns the full release matrix
   across commands, profiles, backends, targets, conformance, and benchmarks.

3. **Diagnosis and remediation are not yet the same system.**
   `doctor` reports some readiness state, but setup/remediation and validation
   remain separate enough that users can still discover missing toolchains late.

4. **Contributor and user flows drift too easily.**
   Thin wrappers have grown logic over time. That creates code smell, repeated
   environment policy, and cross-platform inconsistency.

## 4. Design Principles

1. **One behavioral authority.**
   `molt` owns behavior. Scripts and packaging wrappers only launch it.

2. **Fast path equals correct path.**
   The easiest workflow must be the release-quality workflow.

3. **No silent degradation.**
   Missing required toolchains, unsupported targets, and intentionally deferred
   dynamic features must raise with concrete remediation.

4. **Cross-platform parity is built in.**
   macOS, Linux, and Windows must expose the same conceptual command surface and
   the same result model, even when remediation commands differ.

5. **Validation is product behavior.**
   End-to-end proof is not an ad hoc shell recipe; it is a first-party command.

6. **Canonical artifact locations only.**
   Setup, doctor, validate, logs, and benchmark outputs must use the repo's
   canonical artifact roots.

## 5. Canonical Command Contract

### 5.1 `molt setup`

`molt setup` becomes the single bootstrap authority.

Responsibilities:

- detect host platform, architecture, and supported target lanes;
- check required and optional toolchains;
- install or print exact remediation commands for missing dependencies;
- emit the canonical env/config shape for Molt development and release work;
- prepare the machine for:
  - native / Cranelift;
  - native / LLVM;
  - linked wasm;
  - conformance and benchmark flows.

Expected UX:

- clear platform-specific action plan;
- explicit distinction between required vs optional capabilities;
- no ambiguous "something is missing" output;
- machine-readable JSON output alongside human-readable guidance.

### 5.2 `molt doctor`

`molt doctor` becomes the single readiness and diagnostics authority.

Responsibilities:

- report readiness by:
  - host platform / arch;
  - backend (`native`, `llvm`, `wasm`);
  - target/profile combination;
  - full-validation lane readiness;
- classify each issue as:
  - error;
  - warning;
  - optional capability;
- emit exact fix commands and environment expectations;
- explain external-only blockers distinctly from first-party code failures.

Expected UX:

- deterministic, dense output;
- exact failure ownership;
- actionable remediation;
- JSON suitable for CI and release tooling.

### 5.3 `molt validate`

`molt validate` becomes the single end-to-end proof authority.

Default contract:

- runs the canonical release-readiness matrix for the local machine;
- fails hard on correctness regressions;
- records benchmark evidence and conformance evidence under canonical artifact
  roots;
- supports JSON output for automation.

Required matrix dimensions:

- **Commands**
  - build
  - run
  - compare
  - JSON command surfaces
- **Profiles**
  - dev
  - release
- **Backends**
  - native / Cranelift
  - LLVM
  - linked wasm
- **Correctness suites**
  - focused backend parity
  - conformance suite
  - dynamic-policy enforcement checks
- **Performance**
  - benchmark suite with CPython baseline comparison

Scoped execution must also be supported, for example:

- `molt validate --backend llvm`
- `molt validate --profile release`
- `molt validate --suite conformance`
- `molt validate --suite bench`
- `molt validate --json`

## 6. Thin Wrapper Policy

The following files become thin delegates only:

- `packaging/install.sh`
- `packaging/install.ps1`
- `tools/dev.py`

Constraints:

- no duplicated dependency resolution logic;
- no separate environment policy;
- no independent validation matrix;
- no platform-specific behavioral drift beyond shell syntax and launcher glue.

Allowed behavior:

- resolve launcher-specific path/bootstrap concerns;
- invoke `molt setup`, `molt doctor`, or `molt validate`;
- pass through user arguments.

## 7. Config And Environment Contract

DX must expose one canonical environment story.

`molt setup` and `molt doctor` should recognize and explain:

- `MOLT_EXT_ROOT`
- `CARGO_TARGET_DIR`
- `MOLT_DIFF_CARGO_TARGET_DIR`
- `MOLT_CACHE`
- `MOLT_DIFF_ROOT`
- `MOLT_DIFF_TMPDIR`
- `UV_CACHE_DIR`
- `TMPDIR`
- `MOLT_SESSION_ID`

Recommended direction:

- add a first-party `molt env` subcommand as the readable/machine-readable
  environment authority;
- keep repo-local Cargo policy in `.cargo/config.toml`;
- avoid introducing a second semantic authority in packaging config.

## 8. End-To-End Validation Matrix

`molt validate` must encode the canonical local matrix that Tasks 6 and 7
proved manually.

Minimum must-pass lanes:

1. native `dev` command-level proof
2. native `release` command-level proof
3. LLVM `release` proof for the covered backend slice
4. linked wasm command/runtime proof
5. focused parity tests:
   - native loop/join semantics
   - wasm control-flow parity
   - wasm class smoke
6. CLI command matrix:
   - `run --json`
   - `build` + execute produced binary
   - `compare --json`
7. conformance suite entrypoint
8. benchmark comparison entrypoint against CPython

The benchmark lane should distinguish:

- unavailable benchmark prerequisites;
- benchmark execution failure;
- performance regression against the configured threshold.

## 9. Cross-Platform Contract

The command model must be identical across:

- macOS
- Linux
- Windows

Platform-specific differences are allowed only in:

- installation mechanism;
- remediation commands;
- capability availability.

The semantics of `setup`, `doctor`, and `validate` must not diverge.

## 10. Documentation Contract

When this design lands, the same change set must update:

- `docs/spec/STATUS.md`
- `ROADMAP.md`
- `docs/DEVELOPER_GUIDE.md`
- `docs/OPERATIONS.md`
- `CONTRIBUTING.md`

The docs must point to the canonical CLI contract and stop advertising
parallel script-owned workflows.

## 11. File Map

| Path | Responsibility |
| --- | --- |
| `src/molt/cli.py` | canonical `setup`, `doctor`, `validate`, and shared environment/config logic |
| `tools/dev.py` | thin delegate for contributor convenience only |
| `packaging/install.sh` | thin Unix bootstrap wrapper |
| `packaging/install.ps1` | thin Windows bootstrap wrapper |
| `packaging/config.toml` | packaging metadata only; not behavioral ownership |
| `.cargo/config.toml` | canonical repo-local Cargo policy |
| `docs/DEVELOPER_GUIDE.md` | developer-facing DX contract |
| `docs/OPERATIONS.md` | operations and validation workflow |
| `CONTRIBUTING.md` | contributor-facing command guidance |
| `docs/spec/STATUS.md` | current status of the unified DX migration |
| `ROADMAP.md` | forward-looking DX follow-up and benchmark/conformance closure |

## 12. Acceptance Criteria

This design is complete only when:

1. `molt setup` exists and owns bootstrap/setup behavior;
2. `molt doctor` reports readiness by backend/target/profile with exact fixes;
3. `molt validate` runs the canonical end-to-end matrix;
4. wrapper/install/dev surfaces are thin delegates;
5. docs point at the canonical CLI contract;
6. end-to-end proof is collected on the new path.
