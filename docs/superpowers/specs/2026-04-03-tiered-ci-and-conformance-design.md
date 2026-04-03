# Molt Tiered CI And Canonical Conformance: Cheap Presubmit, Local Authority

**Status:** Approved
**Date:** 2026-04-03
**Scope:** GitHub workflow topology and the canonical Molt conformance lane
**Primary goal:** Keep GitHub validation affordable and high-signal while local
verification remains the full source of truth

---

## 1. Goal

Redesign Molt's validation system around a simple split:

1. local machine = full authoritative presubmit;
2. GitHub push/PR CI = cheap required gates only;
3. GitHub hosted heavy validation = targeted correctness, release, and perf lanes;
4. one canonical Molt conformance runner = the same command locally and in CI.

This should reduce GitHub cost, keep release-relevant hosted validation, and
remove drift between local and CI correctness paths.

## 2. Non-Goals

- Replacing GitHub Actions entirely.
- Keeping every current CI job required on every push.
- Building a large new harness framework just to wrap existing commands.
- Creating separate local and CI implementations of Molt conformance.
- Preserving current workflow structure for backwards compatibility.

## 3. Problem Statement

Molt currently has two structural problems:

1. **GitHub is doing too much on every push.**
   Expensive hosted jobs, especially on `macos-14`, are too costly for a
   cash-strapped solo workflow when they run for routine commits.

2. **Correctness authority is too fragmented.**
   Local validation, harness layers, and GitHub workflows do not yet cleanly
   point at one canonical Molt conformance entrypoint with one result model and
   one environment model.

That combination is the wrong shape:

- it burns hosted budget on work that should be local;
- it makes correctness harder to reason about;
- it encourages CI-only logic;
- it makes the repo feel more complicated than it needs to.

## 4. Design Principles

1. **Local-first authority.**
   The developer machine is the canonical full gatekeeper. GitHub is a backstop,
   not the place where the real answer is first discovered.

2. **Hosted budget is part of architecture.**
   CI topology must reflect cost. Cheap jobs run always. Expensive jobs run only
   when they change a decision.

3. **Few obvious commands.**
   The system should feel closer to Go and Rust tooling: a small number of
   obvious entrypoints, not a maze of semi-overlapping wrappers.

4. **One correctness runner.**
   Molt conformance uses one canonical command, one environment builder, one
   result taxonomy, and one exit policy.

5. **No CI-only semantics.**
   GitHub must call the same correctness command that developers run locally.

6. **Linux by default, macOS when it matters.**
   Presubmit jobs should prefer `ubuntu-latest`. `macos-14` should be reserved
   for release-relevant packaging and host-specific validation.

## 5. Canonical Validation Model

### 5.1 Local authoritative commands

The local machine owns the full answer to “is this ready?” with a small command
surface:

- `python3 tools/dev.py lint`
- `python3 tools/dev.py test`
- `python3 tests/harness/run_molt_conformance.py --suite smoke`
- `python3 tests/harness/run_molt_conformance.py --suite full`

Area-specific targeted commands may still exist, but these are the canonical
top-level entrypoints.

### 5.2 GitHub classes

GitHub validation is split into four workflow classes. These classes may be
implemented as three or four workflow YAML files; the invariant is the class
split, not exact file count.

#### A. Presubmit

- **Trigger:** `push`, `pull_request`
- **Cost target:** cheap
- **Required:** yes
- **Runner policy:** Linux only unless a job is impossible to validate on Linux

Jobs:

- docs/policy gates;
- Python/tooling smoke;
- Rust build/unit smoke;
- optional tiny Molt conformance smoke once the canonical smoke suite exists.

Presubmit must never include full differential, full benchmark, or broad macOS
release validation.

#### B. Nightly correctness

- **Trigger:** `schedule`, `workflow_dispatch`
- **Cost target:** correctness-heavy
- **Required on push:** no

Jobs:

- full Molt conformance;
- differential correctness lane for `tests/differential/basic` and
  `tests/differential/stdlib`.

This workflow exists because semantic drift should be caught automatically, but
not at push-time hosted cost.

#### C. Release validation

- **Trigger:** tags, `workflow_dispatch`
- **Cost target:** release-relevant hosted validation
- **Required on push:** no

Jobs:

- macOS release build/package validation;
- Linux release build/package validation;
- any artifact publication checks.

This keeps GitHub focused on hosted cross-platform work that the local machine
cannot fully replace.

#### D. Perf validation

- **Trigger:** `workflow_dispatch`, optionally weekly schedule
- **Cost target:** expensive, evidence-producing
- **Required on push:** no

Jobs:

- benchmark suite;
- perf artifact/report upload;
- optional benchmark comparisons against checked-in baselines.

Performance evidence remains available without paying its full cost on every
commit.

## 6. Presubmit And Hosted Runner Policy

### 6.1 Presubmit policy

Push/PR CI should contain only the jobs that are both:

1. cheap enough to run every time;
2. valuable enough to block every routine integration.

That means:

- docs architecture and generated-doc freshness;
- Python smoke/lint policy checks that are affordable and actionable;
- Rust build/unit smoke on Linux.

It does **not** mean:

- broad benchmark execution;
- full Monty corpus on every push;
- macOS release packaging;
- broad hosted matrix testing.

### 6.2 Hosted runner defaults

- Default hosted runner for presubmit: `ubuntu-latest`
- Default hosted runner for nightly correctness: `ubuntu-latest` unless a
  correctness lane requires a different host
- `macos-14` is reserved for release and host-specific packaging validation

Any new job that requests macOS must justify why Linux is insufficient.

## 7. Canonical Molt Conformance Lane

### 7.1 Single command

The canonical Molt conformance runner remains:

- `python3 tests/harness/run_molt_conformance.py`

It gains one explicit suite selector:

- `--suite smoke`
- `--suite full`

The smoke suite is the presubmit-friendly subset.
The full suite is the nightly/manual correctness suite.

### 7.2 Suite definition

The smoke suite should be driven by a committed manifest file, for example:

- `tests/harness/corpus/monty_compat/SMOKE.txt`

The full suite is the entire committed Monty compatibility corpus.

Contract:

- one repo-relative test path per line;
- ordering is preserved and defines execution order;
- comments and blank lines are allowed;
- updates are manual and code-reviewed, not auto-generated;
- the file should stay small, stable, and representative enough for push-time
  smoke validation.

This keeps suite selection explicit, reviewable, deterministic, and hard to
silently widen.

### 7.3 Canonical result taxonomy

The runner must use one result model everywhere:

- `passed`
- `failed`
- `compile_error`
- `timeout`
- `skipped`

The canonical summary artifact should contain, at minimum:

- suite name;
- corpus root or manifest path;
- total count;
- counts for each result class;
- total duration;
- per-test failures/compile errors/timeouts.

The runner exits nonzero if any `failed`, `compile_error`, or `timeout` case is
present.

### 7.4 Canonical environment builder

The runner and any harness layer that invokes Molt conformance must share one
environment construction path:

- canonical repo-local cache/tmp/target roots;
- canonical `PYTHONPATH`;
- canonical `MOLT_SESSION_ID`;
- required directory creation.

This logic must not be duplicated independently in:

- `tests/harness/run_molt_conformance.py`
- `src/molt/harness_layers.py`
- GitHub workflow YAML

Instead, a small shared Python utility should own the environment builder and
summary/result serialization.

Canonical location:

- `src/molt/harness_conformance.py`

Minimum API boundary:

- `build_molt_conformance_env(project_root: Path, session_id: str) -> dict[str, str]`
- `ensure_molt_conformance_dirs(env: dict[str, str]) -> None`
- `load_molt_conformance_suite(corpus_dir: Path, suite: str, smoke_manifest: Path) -> list[Path]`
- `write_molt_conformance_summary(path: Path, summary: dict[str, object]) -> None`
- `conformance_exit_code(summary: dict[str, object]) -> int`

The runner, harness layers, and any future CI wrapper must use this module
instead of inventing parallel helpers.

## 8. Harness Integration

`src/molt/harness_layers.py` must not reimplement Molt conformance semantics.

Its conformance/deep layer should delegate to the canonical runner and consume
the canonical result artifact. That gives Molt exactly one definition of:

- suite selection;
- environment setup;
- exit policy;
- pass/fail accounting.

The harness remains a coordinator, not a second correctness implementation.

## 9. Documentation And Status Ownership

After this split lands, the documentation contract should be:

- `tools/dev.py` documents the local full-gate entrypoints;
- CI docs explain workflow classes and trigger policy;
- `docs/spec/STATUS.md` links to the canonical conformance lane as the current
  correctness authority;
- no document should describe GitHub push CI as the authoritative full gate.

The language should stay simple:

- local = full authority;
- GitHub presubmit = cheap required backstop;
- nightly = correctness-heavy automation;
- tags/manual = release and perf validation.

## 10. Acceptance Criteria

This design is complete when all of the following are true:

1. Push/PR CI contains only cheap required gates and no full perf lane.
2. Release-relevant hosted validation still exists in GitHub.
3. A nightly/manual correctness workflow runs the full canonical Molt
   conformance lane.
4. `tests/harness/run_molt_conformance.py` supports `--suite smoke|full`.
5. The smoke suite is defined by a committed deterministic manifest.
6. The runner emits one canonical summary artifact and one canonical exit policy.
7. Harness integration consumes the canonical conformance runner instead of
   reimplementing its logic.
8. Local docs and CI docs consistently describe local verification as the full
   gatekeeper.

## 11. Simplicity Constraints

To keep the system streamlined:

- do not introduce a second top-level conformance CLI if the existing runner can
  be upgraded cleanly;
- do not add CI-only wrapper scripts unless local developers also use them;
- do not require macOS for routine presubmit;
- do not create multiple overlapping smoke suites;
- do not create separate result schemas for local, harness, and GitHub use.

The desired shape is boring and obvious:

- a few workflows;
- a few commands;
- one conformance runner;
- one source of truth.
