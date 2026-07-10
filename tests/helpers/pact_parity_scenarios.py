"""Fail-loud scenario battery for `collab/pact/parity/check_parity.py`.

This module is executed as a STANDALONE SCRIPT inside a `uv run --no-project
--with numpy==1.26.4 python ...` subprocess (see
`tests/tools/test_pact_parity_engine.py`) — never imported directly by the
main pytest process. That mirrors `tools/pact_witness_acceptance.py` /
`tools/pact_witness_oracle.py`, which likewise never import numpy in-process
and only ever reach it through an ephemeral `uv --with` child. The base `dev`
dependency group does not include numpy/scipy (see `pyproject.toml`
`[dependency-groups]`), so importing numpy unconditionally at module scope in
a collected test file would make suite collection depend on incidental
environment state; running the numpy-touching half of the proof in a
dedicated child process avoids that entirely.

Each `scenario_*` function proves BOTH halves of the fail-loud contract in one
shot: the TRUE (uncorrupted) reference/candidate pair PASSES, and the
INJECTED corruption FAILS loudly — never silently skipped, never a widened
tolerance, never a pass-by-default. A scenario raises `AssertionError` on any
violation; `main()` catches that per-scenario and reports a JSON verdict.
"""

from __future__ import annotations

import json
from pathlib import Path
import sys

_ROOT = Path(__file__).resolve().parents[2]
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))

import numpy as np  # noqa: E402

import collab.pact.parity.check_parity as engine  # noqa: E402


def _save(path: Path, **arrays: np.ndarray) -> None:
    np.savez(path, **arrays)


def _save_empty(path: Path) -> None:
    # np.savez with zero arrays still writes a valid (empty) .npz archive.
    np.savez(path)


def _manifest(**outputs: dict) -> dict:
    return {
        "schema_version": 1,
        "kernel": "scenario",
        "status": "ready",
        "outputs": outputs,
    }


def _expect(cond: bool, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


def _expect_pass(verdict: engine.Verdict, label: str) -> None:
    _expect(verdict.ok, f"{label}: true reference must PASS:\n{verdict.report()}")


def _expect_fail(verdict: engine.Verdict, label: str) -> None:
    _expect(
        not verdict.ok,
        f"{label}: corrupted candidate must FAIL loud, never pass-by-default",
    )


# --------------------------------------------------------------------------- #
# 1. missing candidate array
# --------------------------------------------------------------------------- #
def scenario_missing_candidate_array(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    _save(ref, a=np.arange(6, dtype=np.int32).reshape(2, 3))
    _save(cand_ok, a=np.arange(6, dtype=np.int32).reshape(2, 3))
    _save(cand_bad, b=np.zeros((2, 3), dtype=np.int32))  # 'a' entirely absent
    manifest = _manifest(a={"gate": "exact", "dtype": "int32", "shape": [2, 3]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "missing_candidate_array/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "missing_candidate_array")
    _expect(
        len(bad.results) == 1 and bad.results[0].name == "a",
        "missing array must be reported explicitly, never silently dropped from "
        f"the verdict: {bad.report()}",
    )
    _expect(not bad.results[0].ok, f"missing array result must be FAIL: {bad.report()}")
    _expect("MISSING" in bad.results[0].detail, f"detail must say MISSING: {bad.report()}")


# --------------------------------------------------------------------------- #
# 2. unexpected extra candidate array
# --------------------------------------------------------------------------- #
def scenario_extra_candidate_array(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    _save(ref, a=np.arange(4, dtype=np.int32))
    _save(cand_ok, a=np.arange(4, dtype=np.int32))
    _save(cand_bad, a=np.arange(4, dtype=np.int32), surprise=np.zeros(3))
    manifest = _manifest(a={"gate": "exact", "dtype": "int32", "shape": [4]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "extra_candidate_array/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "extra_candidate_array")
    _expect(
        any("surprise" in e and "unexpected" in e for e in bad.errors),
        f"unexpected extra array must be a structural error, not ignored: {bad.errors}",
    )


# --------------------------------------------------------------------------- #
# 3. dtype mismatch
# --------------------------------------------------------------------------- #
def scenario_dtype_mismatch(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    _save(ref, a=np.array([1.0, 2.0, 3.0], dtype=np.float32))
    _save(cand_ok, a=np.array([1.0, 2.0, 3.0], dtype=np.float32))
    _save(cand_bad, a=np.array([1.0, 2.0, 3.0], dtype=np.float64))  # wrong dtype
    manifest = _manifest(a={"gate": "atol", "atol": 1e-3, "dtype": "float32", "shape": [3]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "dtype_mismatch/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "dtype_mismatch")
    [r] = bad.results
    _expect(not r.ok and "dtype" in r.detail, f"expected a dtype-mismatch FAIL: {bad.report()}")


# --------------------------------------------------------------------------- #
# 4. shape mismatch
# --------------------------------------------------------------------------- #
def scenario_shape_mismatch(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    _save(ref, a=np.zeros((4, 4), dtype=np.uint8))
    _save(cand_ok, a=np.zeros((4, 4), dtype=np.uint8))
    _save(cand_bad, a=np.zeros((3, 4), dtype=np.uint8))  # wrong row count
    manifest = _manifest(a={"gate": "exact", "dtype": "uint8", "shape": [4, 4]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "shape_mismatch/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "shape_mismatch")
    [r] = bad.results
    _expect(not r.ok and "shape" in r.detail, f"expected a shape-mismatch FAIL: {bad.report()}")


# --------------------------------------------------------------------------- #
# 5. NaN mask mismatch
# --------------------------------------------------------------------------- #
def scenario_nan_mismatch(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    ref_arr = np.array([1.0, 2.0, np.nan, 4.0], dtype=np.float32)
    _save(ref, a=ref_arr)
    _save(cand_ok, a=ref_arr.copy())
    bad_arr = ref_arr.copy()
    bad_arr[0] = np.nan  # candidate silently turns a finite value into NaN
    _save(cand_bad, a=bad_arr)
    manifest = _manifest(a={"gate": "atol", "atol": 1e-3, "dtype": "float32", "shape": [4]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "nan_mismatch/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "nan_mismatch")
    [r] = bad.results
    _expect(not r.ok and "NaN" in r.detail, f"expected a NaN-mask FAIL: {bad.report()}")


# --------------------------------------------------------------------------- #
# 6. Inf mask / sign mismatch
# --------------------------------------------------------------------------- #
def scenario_inf_mismatch(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    ref_arr = np.array([1.0, np.inf, -np.inf, 4.0], dtype=np.float32)
    _save(ref, a=ref_arr)
    _save(cand_ok, a=ref_arr.copy())
    bad_arr = ref_arr.copy()
    bad_arr[1] = -np.inf  # +Inf flipped to -Inf: a sign mismatch, not a magnitude drift
    _save(cand_bad, a=bad_arr)
    manifest = _manifest(a={"gate": "atol", "atol": 1e-3, "dtype": "float32", "shape": [4]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "inf_mismatch/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "inf_mismatch")
    [r] = bad.results
    _expect(not r.ok and "Inf" in r.detail, f"expected an Inf-mask FAIL: {bad.report()}")


# --------------------------------------------------------------------------- #
# 7. atol is never auto-widened past ATOL_CEILING, and drift beyond a
#    declared (legal) atol is never silently tolerated.
# --------------------------------------------------------------------------- #
def scenario_atol_never_widened(work: Path) -> None:
    # (a) a manifest that DECLARES an atol above the ceiling is refused at
    #     validation time -- structurally, before any array is even loaded.
    too_wide = _manifest(
        a={"gate": "atol", "atol": engine.ATOL_CEILING * 10, "dtype": "float32", "shape": [1]}
    )
    try:
        engine.validate_gates(too_wide)
    except engine.GateSpecError as exc:
        _expect("ATOL_CEILING" in str(exc), f"refusal must cite the ceiling: {exc}")
    else:
        raise AssertionError("a manifest atol above ATOL_CEILING must be REJECTED, not accepted")

    # (b) at exactly the ceiling, a manifest is legal (the ceiling is a
    #     ceiling, not itself forbidden).
    at_ceiling = _manifest(
        a={"gate": "atol", "atol": engine.ATOL_CEILING, "dtype": "float32", "shape": [1]}
    )
    engine.validate_gates(at_ceiling)  # must not raise

    # (c) with a legal atol, drift within tolerance passes and drift beyond it
    #     fails -- the tolerance is exercised for real, never silently ignored.
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    _save(ref, a=np.array([1.0], dtype=np.float32))
    _save(cand_ok, a=np.array([1.0 + 5e-4], dtype=np.float32))  # within 1e-3
    _save(cand_bad, a=np.array([1.0 + 5e-2], dtype=np.float32))  # way beyond 1e-3
    manifest = _manifest(a={"gate": "atol", "atol": 1e-3, "dtype": "float32", "shape": [1]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "atol_never_widened/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "atol_never_widened")
    [r] = bad.results
    _expect(not r.ok, f"drift beyond the declared atol must FAIL: {bad.report()}")


# --------------------------------------------------------------------------- #
# 8. empty / zero-size candidate npz
# --------------------------------------------------------------------------- #
def scenario_empty_zero_size_candidate(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    _save(ref, a=np.arange(4, dtype=np.int32))
    _save(cand_ok, a=np.arange(4, dtype=np.int32))
    _save_empty(cand_bad)
    manifest = _manifest(a={"gate": "exact", "dtype": "int32", "shape": [4]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "empty_zero_size_candidate/true")

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "empty_zero_size_candidate")
    _expect(
        any("empty" in e or "no arrays" in e for e in bad.errors),
        f"an empty candidate npz must be a structural FAIL: {bad.errors}",
    )


# --------------------------------------------------------------------------- #
# 9. a gate that cannot be evaluated (manifest/reference drift) fails, never
#    passes by default.
# --------------------------------------------------------------------------- #
def scenario_unevaluable_manifest_reference_drift(work: Path) -> None:
    ref = work / "ref.npz"
    cand = work / "cand.npz"
    _save(ref, a=np.zeros((4, 4), dtype=np.float32))
    _save(cand, a=np.zeros((4, 4), dtype=np.float32))

    good_manifest = _manifest(a={"gate": "atol", "atol": 1e-3, "dtype": "float32", "shape": [4, 4]})
    _expect_pass(
        engine.check_parity(cand, ref, good_manifest),
        "unevaluable_manifest_reference_drift/true",
    )

    # The manifest claims a dtype the REFERENCE itself does not have -- the
    # manifest has drifted from its own authority. This must fail loud, not
    # silently pass or silently coerce.
    drifted_manifest = _manifest(
        a={"gate": "atol", "atol": 1e-3, "dtype": "float64", "shape": [4, 4]}
    )
    bad = engine.check_parity(cand, ref, drifted_manifest)
    _expect_fail(bad, "unevaluable_manifest_reference_drift(dtype)")
    _expect(
        any("drift" in e for e in bad.errors),
        f"manifest/reference dtype drift must be a structural FAIL: {bad.errors}",
    )

    drifted_shape_manifest = _manifest(
        a={"gate": "atol", "atol": 1e-3, "dtype": "float32", "shape": [5, 5]}
    )
    bad_shape = engine.check_parity(cand, ref, drifted_shape_manifest)
    _expect_fail(bad_shape, "unevaluable_manifest_reference_drift(shape)")
    _expect(
        any("drift" in e for e in bad_shape.errors),
        f"manifest/reference shape drift must be a structural FAIL: {bad_shape.errors}",
    )


# --------------------------------------------------------------------------- #
# 10. bitwise (bit-identical fp32) gate: exact uint8-view byte compare
# --------------------------------------------------------------------------- #
def scenario_bitwise_exact_fp32(work: Path) -> None:
    ref = work / "ref.npz"
    cand_ok = work / "cand_ok.npz"
    cand_bad = work / "cand_bad.npz"
    ref_arr = np.array([1.0, -2.5, 3.25, 0.0], dtype=np.float32)
    _save(ref, a=ref_arr)
    _save(cand_ok, a=ref_arr.copy())
    bad_arr = ref_arr.copy()
    bad_arr[0] = np.nextafter(bad_arr[0], np.float32(2.0))  # +1 ULP
    _save(cand_bad, a=bad_arr)
    manifest = _manifest(a={"gate": "bitwise", "dtype": "float32", "shape": [4]})

    _expect_pass(engine.check_parity(cand_ok, ref, manifest), "bitwise_exact_fp32/true")
    _expect(
        abs(float(bad_arr[0]) - float(ref_arr[0])) < 1e-3,
        "sanity: the +1-ULP corruption must be smaller than any legal atol, "
        "so only a real bitwise compare (not a lenient atol) can catch it",
    )

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "bitwise_exact_fp32")
    [r] = bad.results
    _expect(not r.ok and "byte mismatch" in r.detail, f"expected a byte-level FAIL: {bad.report()}")


# --------------------------------------------------------------------------- #
# 11. order_robust_atol: tolerant of legitimate row permutation, strict on
#     real value drift.
# --------------------------------------------------------------------------- #
def scenario_order_robust_atol(work: Path) -> None:
    ref = work / "ref.npz"
    cand_perm = work / "cand_perm.npz"
    cand_bad = work / "cand_bad.npz"
    ref_arr = np.array(
        [[0, 0, 1.0, 0.0], [1, 1, 0.0, 1.0], [2, 2, 0.7071, 0.7071]], dtype=np.float32
    )
    _save(ref, a=ref_arr)
    _save(cand_perm, a=ref_arr[::-1].copy())  # same rows, reversed order
    bad_arr = ref_arr.copy()
    bad_arr[0, 2] = 0.0  # a real content drift on the (c,r)=(0,0) row
    bad_arr[0, 3] = -1.0
    _save(cand_bad, a=bad_arr)
    manifest = _manifest(
        a={
            "gate": "order_robust_atol",
            "atol": 1e-3,
            "dtype": "float32",
            "shape": [None, 4],
            "key_cols": [0, 1],
        }
    )

    _expect_pass(
        engine.check_parity(cand_perm, ref, manifest),
        "order_robust_atol/true (row-permuted candidate must still PASS)",
    )

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "order_robust_atol")


# --------------------------------------------------------------------------- #
# 12. exact_set: tolerant of legitimate row permutation, strict on real
#     coordinate drift.
# --------------------------------------------------------------------------- #
def scenario_exact_set(work: Path) -> None:
    ref = work / "ref.npz"
    cand_perm = work / "cand_perm.npz"
    cand_bad = work / "cand_bad.npz"
    ref_arr = np.array([[1, 1], [5, 9], [3, 4]], dtype=np.int32)
    _save(ref, a=ref_arr)
    _save(cand_perm, a=ref_arr[::-1].copy())
    bad_arr = ref_arr.copy()
    bad_arr[1] = [99, 99]
    _save(cand_bad, a=bad_arr)
    manifest = _manifest(a={"gate": "exact_set", "dtype": "int32", "shape": [None, 2]})

    _expect_pass(
        engine.check_parity(cand_perm, ref, manifest),
        "exact_set/true (row-permuted candidate must still PASS)",
    )

    bad = engine.check_parity(cand_bad, ref, manifest)
    _expect_fail(bad, "exact_set")


# --------------------------------------------------------------------------- #
# 13. a scaffold manifest can never produce a PASS, independent of content.
# --------------------------------------------------------------------------- #
def scenario_scaffold_manifest_never_passes(work: Path) -> None:
    ref = work / "ref.npz"
    cand = work / "cand.npz"
    _save(ref, a=np.arange(4, dtype=np.int32))
    _save(cand, a=np.arange(4, dtype=np.int32))  # a "perfect" candidate

    scaffold_manifest = {
        "schema_version": 1,
        "kernel": "kernel_c",
        "status": engine.SCAFFOLD_STATUS,
        # Note: 'outputs' below is well-formed and would PASS if evaluated --
        # proving the refusal is unconditional on status, not on content.
        "outputs": {"a": {"gate": "exact", "dtype": "int32", "shape": [4]}},
    }
    try:
        engine.check_parity(cand, ref, scaffold_manifest)
    except engine.GateSpecError as exc:
        _expect("NOT IMPLEMENTED" in str(exc), f"refusal must say NOT IMPLEMENTED: {exc}")
    else:
        raise AssertionError(
            "a scaffold-status manifest must NEVER be evaluated, even with a "
            "perfect candidate and well-formed outputs"
        )


SCENARIOS = {
    "missing_candidate_array": scenario_missing_candidate_array,
    "extra_candidate_array": scenario_extra_candidate_array,
    "dtype_mismatch": scenario_dtype_mismatch,
    "shape_mismatch": scenario_shape_mismatch,
    "nan_mismatch": scenario_nan_mismatch,
    "inf_mismatch": scenario_inf_mismatch,
    "atol_never_widened": scenario_atol_never_widened,
    "empty_zero_size_candidate": scenario_empty_zero_size_candidate,
    "unevaluable_manifest_reference_drift": scenario_unevaluable_manifest_reference_drift,
    "bitwise_exact_fp32": scenario_bitwise_exact_fp32,
    "order_robust_atol": scenario_order_robust_atol,
    "exact_set": scenario_exact_set,
    "scaffold_manifest_never_passes": scenario_scaffold_manifest_never_passes,
}


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    workdir = Path(argv[0]) if argv else Path.cwd()
    workdir.mkdir(parents=True, exist_ok=True)

    results: dict[str, dict[str, object]] = {}
    for name, fn in SCENARIOS.items():
        scenario_dir = workdir / name
        scenario_dir.mkdir(parents=True, exist_ok=True)
        try:
            fn(scenario_dir)
        except Exception as exc:  # noqa: BLE001 - report every failure mode, never swallow
            results[name] = {"ok": False, "detail": f"{type(exc).__name__}: {exc}"}
        else:
            results[name] = {"ok": True, "detail": "PASS"}

    print("RESULT_JSON: " + json.dumps(results))
    return 0 if all(r["ok"] for r in results.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
