# Suite-Honesty Ratchet

Task #46 — the conformance-manifest seed for the 5-year parity axis. Modeled on
the satellite-parity (`tools/check_satellite_parity.py`) and ecosystem-compat
(`tools/check_ecosystem_compat.py`) guards: a fail-closed, **down-only** ratchet
so the differential suite can never read as green while a tracked test is failing,
and can never keep a fixed test on the known-bad list.

> **An entry here is a debt with an owner, not an accepted state.** It leaves only
> by being fixed.

## The problem this kills

An adversarial review proved three differential tests (`kwonly_method_return`,
`classmethod_staticmethod`, `comprehension_lambda_capture`) were failing on base
**silently** — the nightly differential lane ran them and reported the failures in
its `failed_files` list, but no gate went red, so they read as green-ish noise.
Silent failure — and its mirror image, a quietly-fixed test nobody removes from a
known-bad list — is exactly what the parity contract forbids. This ratchet makes
**both** loud.

## What lives here

| File | Role |
|---|---|
| `differential_expectations.json` | **Single source of truth** for every KNOWN-failing differential test, dimensioned by backend (`native`/`llvm`/`wasm`/`luau`) × CPython version. Each `fail` dimension carries `tracking` + `root_cause` + `evidence`. |
| `honesty_baseline.json` | The committed one-way ratchet: per-backend `expected_fail_ceiling` (may only **decrease**). |
| `native_calibration.jsonl` | The committed NATIVE reality snapshot: one JSON line per test with its **raw** status (before the xfail/xpass overlay), produced by `tests/molt_diff.py` with `MOLT_DIFF_RESULTS_JSONL` set. |
| `../check_suite_honesty.py` | The guard. Reconciles the manifest against the snapshot in **both directions** and against the baseline. |

## How a verdict is reached (no hand-asserted greenness)

`tests/molt_diff.py` is the authority on what **happened**: for every test it
records a raw status (`pass`/`fail`/`error`/`oom`/`skip`) and an `expect_molt_fail`
flag. That flag is `True` iff the test is **already tracked by another channel** —
either `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS` (exec/eval/compile, excluded by design)
or an inline `# MOLT_META: expect_fail=molt` marker. Those channels own their
tests; this ratchet owns only the **silent** failures (`expect_molt_fail == False`).
The three channels **partition** the fail space — no test is tracked twice (the
guard's lint enforces it).

This manifest is the authority on what we **expect**: every silent failure must
have a `fail` entry here, and every `fail` entry must still be failing.

## The status vocabulary

| Status | Meaning |
|---|---|
| (absent) | The implicit default: the test is expected to **pass** on every calibrated dimension. The manifest only lists known-bad dimensions, so it stays small and every line is a debt. |
| `fail` | A tracked debt. **Requires** `tracking`, `root_cause`, `evidence`. |
| `uncalibrated` | We have not run this dimension yet and refuse to assert anything. **Loud, never silently absent.** Reality-checks skip it; the lint still validates its shape. |

## What the ratchet enforces (fail-closed)

The guard (`python3 tools/check_suite_honesty.py`) exits non-zero when:

**Manifest lint** (always fatal):
- a dimension status is invalid, or a backend is unknown;
- a `fail` dimension is missing `tracking`, `root_cause`, or `evidence`
  (**anti-parking-lot**: every debt names its owner, a one-line cause, and how it
  was verified — a failure can never be silently parked);
- a test path does not exist on disk (a stale entry can never be matched);
- a test here is **also** in `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS` **or** carries an
  inline `expect_fail=molt` marker (**no parallel truth**).

**Reality check** (both directions, against `native_calibration.jsonl`):
- a **silent** failure (raw `fail`/`error`/`oom`, `expect_molt_fail == False`) with
  no `fail` entry → RED (untracked failure);
- a `fail` entry whose test now **passes** → RED ("remove it — it's fixed");
- a `fail` entry whose test was **skipped** or did not appear → RED (fail-closed:
  the debt cannot be confirmed).

**Baseline ratchet**:
- `expected_fail_ceiling[backend]` rose → RED. It moves **one way only** (down).
  Fixing a test lowers it; a regression that adds a debt is refused.

## How to add an expected-fail entry (you have a new known-bad test)

1. Confirm it is genuinely a fixable debt (not a by-design exclusion — those go in
   `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS`; not an inline-`expect_fail` case).
2. Add it to `differential_expectations.json` under `tests`:
   ```json
   "tests/differential/basic/foo.py": {
     "dimensions": {
       "native": {
         "status": "fail",
         "tracking": "#NN  (or memory/project_xyz_baton.md)",
         "root_cause": "one line: what actually breaks",
         "evidence": "calibrated-run 2026-06-05  (or verified-report <source>)"
       }
     }
   }
   ```
3. Re-run calibration so the snapshot agrees (`--calibrate`, or re-run
   `tests/molt_diff.py` with `MOLT_DIFF_RESULTS_JSONL=tools/suite_honesty/native_calibration.jsonl`).
4. `python3 tools/check_suite_honesty.py --update-baseline` — refused unless the
   ceiling stays flat or **falls**. Adding a debt without fixing one is rejected by
   design; the honest path when a real regression lands is to **fix it**, not widen
   the baseline.

## How to remove an entry (you fixed the test)

1. Fix the test so it matches CPython.
2. Re-calibrate; the test now records raw `pass`.
3. `python3 tools/check_suite_honesty.py` will go **RED** with "remove the entry —
   it's fixed" until you delete the `fail` dimension.
4. Delete it, then `--update-baseline` (the ceiling falls — always allowed).

## Calibrating other backends (llvm / wasm / luau)

The committed snapshot is **native only**; the other backends start `uncalibrated`
in every entry. Calibrating them is tracked as a follow-up (named in the manifest
header). To seed a backend dimension you run that backend's differential lane with
the results sink, then add `verified-evidence` dimensions and re-derive. Never
seed a dimension you have not actually run — mark it `uncalibrated` (loud) instead.

## Commands

```bash
python3 tools/check_suite_honesty.py                 # check vs snapshot+baseline (CI gate)
python3 tools/check_suite_honesty.py --verbose       # + per-backend table
python3 tools/check_suite_honesty.py --show TEST     # one test's expectations
python3 tools/check_suite_honesty.py --lint-only     # manifest lint only (no reality)
python3 tools/check_suite_honesty.py --update-baseline   # down-only
python3 tools/check_suite_honesty.py --reconcile --results FILE
        # rewrite native dims FROM a calibration run (placeholders to fill)
python3 tools/check_suite_honesty.py --calibrate [paths...]
        # run molt_diff to (re)generate native_calibration.jsonl
```

Wired in CI as a `docs-gates` step in `.github/workflows/ci.yml` and as a `lint`
gate in `pyproject.toml`, alongside `check_ecosystem_compat` / `check_dynamic_policy`
/ `check_satellite_parity`. The test suite is `tests/test_check_suite_honesty.py`.

## The relationship to the other expected-fail channels

| Channel | Owns | Tracking | Mechanism |
|---|---|---|---|
| `TOO_DYNAMIC_EXPECTED_FAILURE_TESTS` (`tools/stdlib_full_coverage_manifest.py`) | exec/eval/compile — **excluded by design** | none needed (permanent) | `molt_diff` xfails → resolved `pass` |
| inline `# MOLT_META: expect_fail=molt` | per-test known gaps with an inline `expect_fail_reason` | the inline reason | `molt_diff` xfails → resolved `pass` |
| **this ratchet** | **silent fixable debts** | `tracking` + `root_cause` + `evidence`, machine-checked | down-only manifest gate |

The first two are *runtime suppressions* (they make a fail resolve to a green
`pass` inside `molt_diff`). This ratchet is a *contract* over what remains — the
failures that nobody suppressed and nobody owned. The guard refuses any overlap so
the three never become parallel sources of truth.
