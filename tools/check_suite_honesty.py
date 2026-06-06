#!/usr/bin/env python3
"""Fail-closed, down-only SUITE-HONESTY ratchet (task #46).

The conformance-manifest seed for the 5-year parity axis. Modeled exactly on
tools/check_ecosystem_compat.py and tools/check_satellite_parity.py: a CONTRACT
(not a runner/sync script); a committed source-of-truth manifest; a one-way
ratchet; fail-closed on regression; deterministic sorted output.

The problem this guard exists to kill
-------------------------------------
The differential suite (tests/molt_diff.py) reads as green-ish even when tracked
tests are failing. An adversarial review proved three tests
(kwonly_method_return, classmethod_staticmethod, comprehension_lambda_capture)
were failing on base SILENTLY: no gate went red. Silent failure -- and its mirror
image, a silently-fixed test that nobody removes from the known-bad list -- is the
exact thing the parity contract forbids. This ratchet makes BOTH loud:

  * a test that FAILS without a manifest entry  -> RED (an untracked regression),
  * a manifest expected-fail that now PASSES    -> RED ("remove the entry -- it's
    fixed"; the ratchet is DOWN-ONLY: an entry leaves only by being fixed).

The two manifests
-----------------
  * tools/suite_honesty/differential_expectations.json -- the SINGLE SOURCE OF
    TRUTH for every KNOWN-failing differential test, dimensioned by backend
    (native/llvm/wasm/luau) x CPython version. Each expected-fail entry carries
    `tracking` (task id / baton path), `root_cause` (one line), and `evidence`
    (how it was verified). An entry without all three fails the manifest's own
    lint. A dimension not yet calibrated is marked `"uncalibrated"` -- LOUD,
    never silently absent.
  * tools/suite_honesty/honesty_baseline.json -- the committed one-way ratchet:
    per-dimension `expected_fail_ceiling` (may only DECREASE) so the count of
    known-bad tests can never silently grow.

Reality comes from a CALIBRATION RESULTS file produced by tests/molt_diff.py with
MOLT_DIFF_RESULTS_JSONL set: one JSON line per test with its RAW status (before
the xfail/xpass overlay). The guard NEVER runs the suite itself in --check mode
(separation of concerns + CI determinism); `--calibrate` runs it to regenerate
the snapshot. molt_diff is the authority on what HAPPENED; this manifest is the
authority on what we EXPECT.

The relationship to the existing too-dynamic manifest (no parallel truth)
------------------------------------------------------------------------
tools/stdlib_full_coverage_manifest.py's TOO_DYNAMIC_EXPECTED_FAILURE_TESTS is a
DIFFERENT category: exec/eval/compile tests that are excluded BY DESIGN (the
0215-spec forbids them), permanent, no owner needed -- the analogue of the
ecosystem ratchet's `incompatible-by-design`. This honesty manifest tracks FIXABLE
DEBTS (a fail with a tracking task). To keep the two from becoming parallel
sources of truth, the guard treats the too-dynamic set as authoritative for
by-design exclusions and SUBTRACTS it from the observed-fail set before checking:
a by-design test is never required to appear here, and a debt here may never be a
by-design test. They partition the fail space; neither overlaps the other.

What this guard enforces (fail-closed everywhere)
-------------------------------------------------
Manifest lint (always fatal, mirrors the ecosystem feature-manifest lint):
  * an entry's per-dimension status is outside the allowed set, or
  * a `fail` dimension is missing any of `tracking` / `root_cause` / `evidence`
    (anti-parking-lot: every debt names its owner + cause + how it was verified),
  * a test path does not exist on disk (a stale entry that can never be matched),
  * a test listed here is ALSO in the too-dynamic by-design set (parallel truth).

Against a calibration results file (the reality check, BOTH directions):
  * a test whose RAW status is `fail`/`error`/`oom` for a calibrated dimension,
    that is NOT in the by-design set and has NO manifest `fail` entry for that
    dimension -> RED (untracked failure), or
  * a manifest `fail` entry for a calibrated dimension whose RAW status is now
    `pass` -> RED ("remove the entry -- it's fixed"; down-only).
  * `uncalibrated` dimensions are never reality-checked (no data), but they ARE
    lint-checked for shape.

Against the committed baseline (the one-way ratchet):
  * `expected_fail_ceiling[dim]` may only DECREASE; the guard refuses to raise it
    (same asymmetry as the satellite/ecosystem ceilings). Fixing a test lowers a
    ceiling; a regression that adds a debt would raise it and is refused.

Usage
-----
  python3 tools/check_suite_honesty.py                       # check vs results+baseline
  python3 tools/check_suite_honesty.py --results FILE        # explicit results jsonl
  python3 tools/check_suite_honesty.py --verbose             # + per-dimension table
  python3 tools/check_suite_honesty.py --show TEST           # one test's expectations
  python3 tools/check_suite_honesty.py --lint-only           # manifest lint, no reality
  python3 tools/check_suite_honesty.py --update-baseline      # improving direction only
  python3 tools/check_suite_honesty.py --reconcile --results FILE
        # rewrite manifest+baseline FROM a calibration run, refusing wrong-direction
  python3 tools/check_suite_honesty.py --calibrate [paths...] # run molt_diff -> results
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HONESTY_DIR = Path(__file__).resolve().parent / "suite_honesty"
MANIFEST_PATH = HONESTY_DIR / "differential_expectations.json"
BASELINE_PATH = HONESTY_DIR / "honesty_baseline.json"
# Default calibration snapshot consumed in --check mode (committed so CI is
# deterministic and does not have to rebuild the world on every PR).
DEFAULT_RESULTS_PATH = HONESTY_DIR / "native_calibration.jsonl"
MOLT_DIFF_PATH = ROOT / "tests" / "molt_diff.py"
TOO_DYNAMIC_MANIFEST_PATH = ROOT / "tools" / "stdlib_full_coverage_manifest.py"

# --- the status vocabulary -------------------------------------------------
# A per-dimension expected status. "pass" is the implicit default for any test
# NOT mentioned in the manifest, so the manifest only ever lists KNOWN-bad
# dimensions (it stays small and every line is a debt). "uncalibrated" means we
# have not yet run that dimension and refuse to assert anything (fail-closed:
# loud absence, never silent).
STATUS_FAIL = "fail"
STATUS_UNCALIBRATED = "uncalibrated"
VALID_EXPECTED_STATUSES = {STATUS_FAIL, STATUS_UNCALIBRATED}

# Raw statuses emitted by tests/molt_diff.py (_record_diff_result). Anything in
# FAILING_RAW_STATUSES is a divergence from CPython; "pass" matches; "skip" means
# the test did not run on this host/version (never a verdict).
FAILING_RAW_STATUSES = {"fail", "error", "oom"}
PASSING_RAW_STATUS = "pass"
SKIP_RAW_STATUS = "skip"

# The backend dimensions this ratchet tracks. native is calibrated here; the
# others start `uncalibrated` until their own calibration task runs.
BACKENDS = ("native", "llvm", "wasm", "luau")

# A `fail` entry MUST carry these provenance fields (anti-parking-lot doctrine).
REQUIRED_FAIL_FIELDS = ("tracking", "root_cause", "evidence")


def _rel(path: Path) -> str:
    """Repo-relative display string, falling back to the absolute path when the
    path lives outside ROOT (e.g. a tmp manifest under test). Never raises, so a
    failure message can always be constructed.
    """
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return str(path)


class GuardError(Exception):
    """A manifest-level defect (not a baseline regression). Always fatal."""


def _load_json(path: Path) -> dict:
    if not path.exists():
        raise GuardError(f"manifest missing: {path}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:  # pragma: no cover - exercised via fixtures
        raise GuardError(f"manifest is not valid JSON ({path}): {exc}") from exc


def load_manifest() -> dict:
    data = _load_json(MANIFEST_PATH)
    tests = data.get("tests")
    if not isinstance(tests, dict):
        raise GuardError("differential_expectations.json has no `tests` object")
    return data


def manifest_tests(data: dict) -> dict[str, dict]:
    return data.get("tests", {})


def load_too_dynamic_set() -> frozenset[str]:
    """The by-design exclusion set (exec/eval/compile), from the canonical
    too-dynamic manifest. Authoritative; this ratchet only SUBTRACTS it.
    """
    if not TOO_DYNAMIC_MANIFEST_PATH.exists():
        return frozenset()
    import runpy

    try:
        namespace = runpy.run_path(str(TOO_DYNAMIC_MANIFEST_PATH))
    except Exception as exc:  # pragma: no cover - defensive
        raise GuardError(
            f"could not load too-dynamic manifest {TOO_DYNAMIC_MANIFEST_PATH}: {exc}"
        ) from exc
    raw = namespace.get("TOO_DYNAMIC_EXPECTED_FAILURE_TESTS", ())
    out: set[str] = set()
    for item in raw:
        if isinstance(item, str):
            out.add(_normalize(item))
    return frozenset(out)


def _has_inline_expect_fail(path: str) -> bool:
    """True if the test file carries an inline `# MOLT_META: expect_fail=molt` (or
    `xfail=molt`) marker. That is the SECOND by-design/tracked channel; a test in
    this honesty manifest must not also use it (no parallel truth).
    """
    file_path = ROOT / path
    if not file_path.exists():
        return False
    try:
        text = file_path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return False
    for line in text.splitlines():
        s = line.strip()
        if not s.startswith("# MOLT_META:"):
            continue
        payload = s[len("# MOLT_META:") :].strip().lower()
        for token in payload.split():
            if token.startswith(("expect_fail=", "xfail=")) and "molt" in token:
                return True
    return False


def _normalize(path: str) -> str:
    """Repo-relative POSIX path, matching tests/molt_diff._normalize_repo_relative."""
    candidate = Path(path)
    if not candidate.is_absolute():
        candidate = (ROOT / candidate).resolve()
    else:
        candidate = candidate.resolve()
    try:
        return candidate.relative_to(ROOT).as_posix()
    except ValueError:
        return candidate.as_posix()


# --------------------------------------------------------------------------
# Manifest lint
# --------------------------------------------------------------------------


def validate_manifest(data: dict, too_dynamic: frozenset[str]) -> list[str]:
    """Return a sorted list of manifest defects (fail-closed checks)."""
    problems: list[str] = []
    tests = manifest_tests(data)
    for raw_path in sorted(tests):
        entry = tests[raw_path]
        path = _normalize(raw_path)
        if path != raw_path:
            problems.append(
                f"[{raw_path}] test path is not repo-relative-normalized; expected "
                f"{path!r}. Use the exact path tests/molt_diff.py emits."
            )
        if not (ROOT / path).exists():
            problems.append(
                f"[{path}] test file does not exist on disk. A stale entry can "
                "never be matched against reality -- remove it or fix the path."
            )
        if path in too_dynamic:
            problems.append(
                f"[{path}] is ALSO in TOO_DYNAMIC_EXPECTED_FAILURE_TESTS (by-design "
                "exclusion). The honesty manifest tracks FIXABLE DEBTS only; a "
                "by-design exclusion must not appear here (no parallel truth)."
            )
        if _has_inline_expect_fail(path):
            problems.append(
                f"[{path}] carries an inline `# MOLT_META: expect_fail=molt` marker, "
                "which is a SEPARATE expected-fail channel. A test tracked inline "
                "must not also be tracked here (no parallel truth). Remove one."
            )
        problems += _validate_dimensions(path, entry)
    # The compliance block uses the same dimension shape but a pytest harness; it
    # is lint-validated (anti-parking-lot) but NOT differential-reality-checked.
    for raw_key in sorted(data.get("compliance", {})):
        problems += _validate_dimensions(
            raw_key, data["compliance"][raw_key], is_compliance=True
        )
    return problems


def _validate_dimensions(
    path: str, entry: dict, *, is_compliance: bool = False
) -> list[str]:
    """Validate one entry's `dimensions` object (shared by tests + compliance)."""
    problems: list[str] = []
    dims = entry.get("dimensions")
    if not isinstance(dims, dict) or not dims:
        problems.append(
            f"[{path}] has no `dimensions` object. Every entry must state at "
            "least one (backend[/version]) dimension."
        )
        return problems
    for dim_key in sorted(dims):
        status = dims[dim_key]
        backend = dim_key.split("@", 1)[0]
        if backend not in BACKENDS:
            problems.append(
                f"[{path}] dimension {dim_key!r} has unknown backend "
                f"{backend!r}; allowed backends: {list(BACKENDS)} "
                "(optionally suffixed '@<cpython-version>')."
            )
        if not isinstance(status, dict):
            problems.append(
                f"[{path}] dimension {dim_key!r} must be an object "
                "{status, tracking, root_cause, evidence}."
            )
            continue
        st = status.get("status")
        if st not in VALID_EXPECTED_STATUSES:
            problems.append(
                f"[{path}] dimension {dim_key!r} has invalid status {st!r}; "
                f"allowed: {sorted(VALID_EXPECTED_STATUSES)}. (A passing "
                "dimension is the implicit default -- do not list it.)"
            )
            continue
        if st == STATUS_FAIL:
            for field in REQUIRED_FAIL_FIELDS:
                if not str(status.get(field, "")).strip():
                    problems.append(
                        f"[{path}] dimension {dim_key!r} is `fail` but has empty "
                        f"`{field}`. Anti-parking-lot doctrine: every debt names "
                        "its tracking owner, a one-line root_cause, and the "
                        "evidence it was verified by. A failure can never be "
                        "silently parked."
                    )
    return problems


# --------------------------------------------------------------------------
# Calibration results ingestion
# --------------------------------------------------------------------------


def load_results(path: Path) -> dict[str, dict]:
    """Read a molt_diff results JSONL into {repo_relative_path: record}.

    Each record is {"raw_status": str, "expect_molt_fail": bool}. raw_status is
    Molt's outcome vs CPython before the xfail/xpass overlay; expect_molt_fail is
    True iff the test is ALREADY tracked by another expected-fail channel (the
    too-dynamic manifest OR an inline `# MOLT_META: expect_fail=molt` marker).
    That flag is what partitions the fail space: this honesty ratchet owns only
    the failures with expect_molt_fail == False (the SILENT ones), so it never
    becomes a parallel source of truth with the inline-meta channel.

    If the same test appears twice (retries), the WORST status wins so a flaky
    pass can never mask a fail (fail-closed). Order: error/oom/fail beat pass
    beats skip; expect_molt_fail is OR-ed across rows (any tracked row -> tracked).
    """
    if not path.exists():
        raise GuardError(
            f"calibration results file missing: {path}. Generate it with "
            "tests/molt_diff.py (MOLT_DIFF_RESULTS_JSONL=...) or run --calibrate."
        )
    severity = {"error": 4, "oom": 3, "fail": 2, "pass": 1, "skip": 0}
    out: dict[str, dict] = {}
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as exc:
            raise GuardError(
                f"calibration results line {lineno} is not valid JSON: {exc}"
            ) from exc
        f = row.get("file")
        raw = row.get("raw_status")
        if not isinstance(f, str) or not isinstance(raw, str):
            continue
        key = _normalize(f)
        rec = out.get(key)
        emf = bool(row.get("expect_molt_fail", False))
        if rec is None:
            out[key] = {"raw_status": raw, "expect_molt_fail": emf}
        else:
            if severity.get(raw, 0) > severity.get(rec["raw_status"], 0):
                rec["raw_status"] = raw
            rec["expect_molt_fail"] = rec["expect_molt_fail"] or emf
    if not out:
        raise GuardError(f"calibration results file {path} contained no usable rows")
    return out


def _dim_matches_native(dim_key: str) -> bool:
    """The committed results snapshot is the NATIVE dimension. A manifest
    dimension applies to it when its backend is native (with or without a
    @version suffix). Other backends are not reality-checked from this file.
    """
    return dim_key.split("@", 1)[0] == "native"


def reality_check(
    data: dict,
    results: dict[str, dict],
    too_dynamic: frozenset[str],
    *,
    dim_filter=_dim_matches_native,
    results_backend: str = "native",
) -> list[str]:
    """Both-direction reality check of the manifest against observed results.

    The fail space is partitioned by each result's `expect_molt_fail` flag: a
    test that is already tracked by another expected-fail channel (too-dynamic
    manifest or inline `expect_fail=molt` meta) is OWNED by that channel and is
    never required (nor allowed) here. This ratchet enforces both directions only
    over the SILENT failures (expect_molt_fail == False).
    """
    failures: list[str] = []
    tests = manifest_tests(data)

    # Direction 1: every observed SILENT failing test must have a `fail` manifest
    # entry for this backend.
    expected_fail_paths: set[str] = set()
    for path, entry in tests.items():
        npath = _normalize(path)
        dims = entry.get("dimensions", {})
        if not isinstance(dims, dict):
            continue
        for dim_key, status in dims.items():
            if not dim_filter(dim_key) or not isinstance(status, dict):
                continue
            if status.get("status") == STATUS_FAIL:
                expected_fail_paths.add(npath)

    for path in sorted(results):
        rec = results[path]
        raw = rec["raw_status"]
        if raw not in FAILING_RAW_STATUSES:
            continue
        if rec["expect_molt_fail"]:
            # Already tracked by the too-dynamic manifest or an inline
            # `expect_fail=molt` marker; molt_diff xfails it. Owned by that
            # channel, never by this ratchet (no parallel truth).
            continue
        if path not in expected_fail_paths:
            failures.append(
                f"[{path}] UNTRACKED {results_backend.upper()} FAILURE "
                f"(raw status {raw!r}) with no manifest entry. Either fix it, or "
                "add a tracked expected-fail entry (tracking + root_cause + "
                f"evidence) to {_rel(MANIFEST_PATH)}. Silent failure "
                "is forbidden."
            )

    # Direction 2: every manifest `fail` entry for this backend must still be
    # observed failing; if it now passes, it is FIXED -> remove it (down-only).
    for path in sorted(expected_fail_paths):
        rec = results.get(path)
        raw = rec["raw_status"] if rec is not None else None
        if raw is None:
            # The test did not appear in the results at all. Fail-closed: a
            # manifest debt whose test was not even run is suspicious (renamed?
            # deleted? excluded?). Flag it so the entry can never silently rot.
            failures.append(
                f"[{path}] manifest expects a {results_backend.upper()} FAIL but "
                "the test did not appear in the calibration results (not run / "
                "renamed / deleted?). Re-calibrate, or fix the manifest path."
            )
            continue
        if raw == PASSING_RAW_STATUS:
            failures.append(
                f"[{path}] manifest expects a {results_backend.upper()} FAIL but "
                "the test now PASSES -- it is FIXED. Remove the entry from "
                f"{_rel(MANIFEST_PATH)} (this ratchet is DOWN-ONLY: "
                "entries leave only by being fixed)."
            )
        elif raw == SKIP_RAW_STATUS:
            failures.append(
                f"[{path}] manifest expects a {results_backend.upper()} FAIL but "
                "the test was SKIPPED in calibration (host/version gate?). A "
                "skipped test cannot confirm the debt -- re-calibrate on a host "
                "that runs it, or narrow the dimension."
            )
        # raw in FAILING_RAW_STATUSES -> still failing, as expected. OK.
    return failures


# --------------------------------------------------------------------------
# Baseline (the one-way ratchet on the count of known-bad dimensions)
# --------------------------------------------------------------------------


def fail_ceilings(data: dict) -> dict[str, int]:
    """Per-backend count of `fail` dimensions across all tests (the debt size)."""
    counts = {b: 0 for b in BACKENDS}
    for entry in manifest_tests(data).values():
        dims = entry.get("dimensions", {})
        if not isinstance(dims, dict):
            continue
        for dim_key, status in dims.items():
            if not isinstance(status, dict) or status.get("status") != STATUS_FAIL:
                continue
            backend = dim_key.split("@", 1)[0]
            if backend in counts:
                counts[backend] += 1
    return counts


def load_baseline() -> dict:
    if not BASELINE_PATH.exists():
        return {}
    return json.loads(BASELINE_PATH.read_text(encoding="utf-8"))


def _baseline_payload(data: dict) -> dict:
    counts = fail_ceilings(data)
    return {
        "_comment": (
            "Fail-closed, DOWN-ONLY suite-honesty baseline. Generated by "
            "tools/check_suite_honesty.py --update-baseline. expected_fail_ceiling "
            "is the count of KNOWN-failing dimensions per backend; it may only "
            "DECREASE (fixing a test lowers it; the guard refuses to raise it). "
            "Every fail entry in differential_expectations.json is a debt with an "
            "owner (tracking + root_cause + evidence). See the script docstring "
            "and tools/suite_honesty/README.md."
        ),
        "expected_fail_ceiling": counts,
    }


def check_baseline(data: dict, baseline: dict) -> list[str]:
    failures: list[str] = []
    counts = fail_ceilings(data)
    ceilings = baseline.get("expected_fail_ceiling", {})
    for backend in BACKENDS:
        ceiling = ceilings.get(backend)
        if ceiling is None:
            continue
        if counts[backend] > ceiling:
            failures.append(
                f"expected_fail_ceiling[{backend}] exceeded: {counts[backend]} "
                f"known-bad dimensions > committed ceiling {ceiling}. A new debt "
                "was added without fixing one. This ratchet is DOWN-ONLY: fix a "
                "test (lowering the ceiling) instead of widening the baseline."
            )
    return failures


def cmd_update_baseline() -> int:
    data = load_manifest()
    too_dynamic = load_too_dynamic_set()
    problems = validate_manifest(data, too_dynamic)
    if problems:
        print(
            "REFUSING to update baseline: the manifest has defects that must be "
            "fixed first:\n",
            file=sys.stderr,
        )
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1
    new = _baseline_payload(data)
    prev = load_baseline()
    if prev:
        prev_ceilings = prev.get("expected_fail_ceiling", {})
        for backend in BACKENDS:
            prev_v = prev_ceilings.get(backend)
            if prev_v is not None and new["expected_fail_ceiling"][backend] > prev_v:
                print(
                    f"REFUSING to raise expected_fail_ceiling[{backend}] "
                    f"{prev_v} -> {new['expected_fail_ceiling'][backend]}. A "
                    "regression added a debt. Fix it instead of widening the "
                    "baseline (the ratchet is one-way toward zero).",
                    file=sys.stderr,
                )
                return 1
    BASELINE_PATH.write_text(
        json.dumps(new, indent=2, sort_keys=False) + "\n", encoding="utf-8"
    )
    counts = new["expected_fail_ceiling"]
    print(
        "baseline updated: expected_fail_ceiling="
        + ", ".join(f"{k}={v}" for k, v in counts.items())
    )
    return 0


# --------------------------------------------------------------------------
# check / show / lint
# --------------------------------------------------------------------------


def cmd_check(results_path: Path, verbose: bool) -> int:
    try:
        data = load_manifest()
        too_dynamic = load_too_dynamic_set()
    except GuardError as exc:
        print(f"\nSUITE HONESTY GUARD FAILED:\n  - {exc}\n", file=sys.stderr)
        return 1
    problems = validate_manifest(data, too_dynamic)

    results: dict[str, str] | None = None
    try:
        results = load_results(results_path)
    except GuardError as exc:
        problems.append(str(exc))

    if results is not None:
        problems += reality_check(data, results, too_dynamic)

    baseline = load_baseline()
    if not baseline:
        problems.append(
            f"no committed baseline at {_rel(BASELINE_PATH)}. Run "
            "--update-baseline once to establish the ratchet."
        )
    else:
        problems += check_baseline(data, baseline)

    if verbose:
        counts = fail_ceilings(data)
        print(f"{'backend':<10} {'known-bad dims':>14}")
        for backend in BACKENDS:
            print(f"{backend:<10} {counts[backend]:>14}")
        print()
        if results is not None:
            obs_fail = sum(
                1
                for rec in results.values()
                if rec["raw_status"] in FAILING_RAW_STATUSES
                and not rec["expect_molt_fail"]
            )
            print(
                f"calibration {results_path.name}: {len(results)} tests, "
                f"{obs_fail} SILENT (untracked-channel) failures observed"
            )

    if problems:
        print("\nSUITE HONESTY GUARD FAILED:\n", file=sys.stderr)
        for p in sorted(problems):
            print(f"  - {p}", file=sys.stderr)
        print(
            "\nThe differential suite's known-state manifest "
            f"({_rel(MANIFEST_PATH)}) disagrees with reality, or a "
            "debt lacks an owner, or the ratchet regressed. Every failing test "
            "must be either fixed or a TRACKED expected-fail (tracking + "
            "root_cause + evidence); every fixed test must be REMOVED from the "
            "manifest. See tools/suite_honesty/README.md.\n",
            file=sys.stderr,
        )
        return 1
    counts = fail_ceilings(data)
    print(
        "suite honesty OK: "
        + ", ".join(f"{k}={v}" for k, v in counts.items())
        + f" known-bad dims; {len(manifest_tests(data))} tracked tests "
        f"within baseline (calibration {results_path.name})."
    )
    return 0


def cmd_lint_only() -> int:
    data = load_manifest()
    too_dynamic = load_too_dynamic_set()
    problems = validate_manifest(data, too_dynamic)
    if problems:
        print("\nSUITE HONESTY MANIFEST LINT FAILED:\n", file=sys.stderr)
        for p in sorted(problems):
            print(f"  - {p}", file=sys.stderr)
        return 1
    print(f"manifest lint OK: {len(manifest_tests(data))} tracked tests.")
    return 0


def cmd_show(test: str) -> int:
    data = load_manifest()
    tests = manifest_tests(data)
    npath = _normalize(test)
    entry = tests.get(npath) or tests.get(test)
    if entry is None:
        print(
            f"no manifest entry for {npath!r} (so it is expected to PASS on every "
            "calibrated dimension).",
            file=sys.stderr,
        )
        return 2
    print(f"# test {npath}")
    dims = entry.get("dimensions", {})
    for dim_key in sorted(dims):
        st = dims[dim_key]
        if not isinstance(st, dict):
            print(f"#   {dim_key}: {st!r} (MALFORMED)")
            continue
        print(f"#   {dim_key}: {st.get('status')}")
        if st.get("status") == STATUS_FAIL:
            print(f"#       tracking:   {st.get('tracking')}")
            print(f"#       root_cause: {st.get('root_cause')}")
            print(f"#       evidence:   {st.get('evidence')}")
    return 0


# --------------------------------------------------------------------------
# calibrate (run molt_diff to produce a results snapshot) + reconcile
# --------------------------------------------------------------------------


def cmd_calibrate(paths: list[str], results_out: Path, jobs: int, profile: str) -> int:
    if not MOLT_DIFF_PATH.exists():
        print(f"tests/molt_diff.py not found at {MOLT_DIFF_PATH}", file=sys.stderr)
        return 1
    targets = paths or ["tests/differential/basic", "tests/differential/stdlib"]
    results_out.parent.mkdir(parents=True, exist_ok=True)
    if results_out.exists():
        results_out.unlink()
    env = dict(os.environ)
    env["MOLT_DIFF_RESULTS_JSONL"] = str(results_out)
    cmd = [
        sys.executable,
        "-u",
        str(MOLT_DIFF_PATH),
        "--build-profile",
        profile,
        "--jobs",
        str(jobs),
        *targets,
    ]
    print(f"calibrating: {' '.join(cmd)}", file=sys.stderr)
    proc = subprocess.run(cmd, env=env)
    if not results_out.exists():
        print(
            f"calibration produced no results file at {results_out}",
            file=sys.stderr,
        )
        return 1
    print(
        f"calibration results written to {results_out} (molt_diff exit {proc.returncode})"
    )
    return 0


def cmd_reconcile(results_path: Path) -> int:
    """Rewrite the manifest's NATIVE dimensions + baseline from a calibration run.

    This is the ONLY way to seed/refresh the native dimension. It is NOT a free
    pass to widen the manifest: the down-only baseline ratchet still refuses a
    rewrite that raises a ceiling. Entries gain a placeholder provenance the
    author MUST fill (the lint then forces tracking/root_cause/evidence before
    --update-baseline accepts it).
    """
    try:
        data = load_manifest()
    except GuardError:
        data = {"tests": {}}
    results = load_results(results_path)
    tests = data.setdefault("tests", {})

    observed_fail = {
        p
        for p, rec in results.items()
        if rec["raw_status"] in FAILING_RAW_STATUSES and not rec["expect_molt_fail"]
    }
    # Remove native fail-dims for tests that now pass (down-only fix detection).
    removed: list[str] = []
    for path in sorted(list(tests)):
        entry = tests[path]
        dims = entry.get("dimensions", {})
        npath = _normalize(path)
        if "native" in dims and dims["native"].get("status") == STATUS_FAIL:
            if npath not in observed_fail:
                del dims["native"]
                removed.append(npath)
                if not dims:
                    del tests[path]
    # Add native fail-dims for newly observed failures, with placeholder
    # provenance the author must complete.
    added: list[str] = []
    for path in sorted(observed_fail):
        entry = tests.setdefault(path, {"dimensions": {}})
        dims = entry.setdefault("dimensions", {})
        if "native" not in dims:
            dims["native"] = {
                "status": STATUS_FAIL,
                "tracking": "",
                "root_cause": f"raw status {results[path]['raw_status']!r} -- FILL IN",
                "evidence": f"calibration {results_path.name} -- FILL IN tracking",
            }
            added.append(path)

    MANIFEST_PATH.write_text(
        json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(
        f"reconciled native dimension: +{len(added)} new fail entries, "
        f"-{len(removed)} fixed entries removed.",
        file=sys.stderr,
    )
    if added:
        print("  NEW entries (fill in tracking/root_cause/evidence):", file=sys.stderr)
        for p in added:
            print(f"    {p}", file=sys.stderr)
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--results", help="calibration results JSONL (default: committed snapshot)"
    )
    ap.add_argument(
        "--verbose", action="store_true", help="print the per-backend table"
    )
    ap.add_argument("--show", metavar="TEST", help="print one test's expectations")
    ap.add_argument(
        "--lint-only", action="store_true", help="manifest lint only (no reality)"
    )
    ap.add_argument(
        "--update-baseline",
        action="store_true",
        help="regenerate the baseline (improving direction only)",
    )
    ap.add_argument(
        "--reconcile",
        action="store_true",
        help="rewrite native dims+baseline FROM a calibration run",
    )
    ap.add_argument(
        "--calibrate",
        action="store_true",
        help="run molt_diff to produce a results snapshot",
    )
    ap.add_argument(
        "--calibrate-out",
        help="where --calibrate writes results (default: committed snapshot)",
    )
    ap.add_argument(
        "--jobs", type=int, default=4, help="parallel jobs for --calibrate (default 4)"
    )
    ap.add_argument(
        "--profile", default="dev", help="build profile for --calibrate (default dev)"
    )
    ap.add_argument("paths", nargs="*", help="differential paths for --calibrate")
    args = ap.parse_args(argv)

    results_path = (
        Path(args.results).expanduser() if args.results else DEFAULT_RESULTS_PATH
    )

    try:
        if args.show:
            return cmd_show(args.show)
        if args.lint_only:
            return cmd_lint_only()
        if args.calibrate:
            out = (
                Path(args.calibrate_out).expanduser()
                if args.calibrate_out
                else DEFAULT_RESULTS_PATH
            )
            return cmd_calibrate(args.paths, out, args.jobs, args.profile)
        if args.reconcile:
            return cmd_reconcile(results_path)
        if args.update_baseline:
            return cmd_update_baseline()
        return cmd_check(results_path, args.verbose)
    except GuardError as exc:
        print(f"\nSUITE HONESTY GUARD FAILED:\n  - {exc}\n", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
