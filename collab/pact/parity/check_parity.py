#!/usr/bin/env python3
"""Generalized, FAIL-LOUD parity oracle for the pact witness kernel suite.

This is the single shared acceptance engine molt owes pact per `009 §5` and the
`011` parity-harness proposal (`collab/pact/011_molt_reply_progress_sync_and_harness_proposal_20260710.md`
§2). It generalizes Kernel A's previously-inline `check_parity.py` gate dicts into
a declarative per-kernel manifest (`<k>_gates.json`) and a single strict verdict
engine so Kernels B..7 drop in with ZERO engine changes.

    python check_parity.py candidate.npz reference.npz <k>_gates.json

New kernels (B..7) drop in as a `<k>.py` / `make_<k>_fixture.py` / `<k>_gates.json`
file-set with zero changes to this engine; `make_kernel_scaffold.py` in this same
directory emits a loud, un-passable NOT-IMPLEMENTED scaffold for a kernel awaiting
pact source (its manifest carries `status: "AWAITING_PACT_KERNEL_SOURCE"`, which is
refused unconditionally below -- see `SCAFFOLD_STATUS`).

Exit codes:
    0  every gate PASSED (strict verdict)
    1  a parity gate FAILED (precise per-array diff printed)
    2  the run could not be evaluated (missing file, unreadable npz, invalid or
       scaffold manifest) — a structural refusal, never a pass-by-default.

FAIL-LOUD guarantees (non-negotiable — M05 / 006 / 009 / 011). The engine FAILS,
never silently passes, on every one of these:

  * a manifest output array MISSING from the candidate                (never skip)
  * an UNEXPECTED extra array in the candidate (keyset must match)
  * a dtype mismatch (candidate vs the manifest-declared dtype)
  * a shape mismatch (candidate vs manifest fixed dims / reference-defined dims)
  * a NaN mask mismatch or an Inf mask/sign mismatch on a float gate
  * an empty / zero-array candidate npz
  * a gate that cannot be evaluated (unknown gate, missing atol/key_cols,
    reference/manifest drift) — a hard structural FAIL, never a pass
  * a +1-ULP change under a `bitwise` (bit-identical fp32) gate
  * a float drift beyond the per-gate `atol`

Tolerance is PER-GATE and is NEVER auto-widened: `atol` may not exceed
`ATOL_CEILING` (1e-3). A manifest that declares a wider atol is REJECTED at
validation time (exit 2), structurally preventing the "widen to go green" poison.

Gate classes (declared per output array in `<k>_gates.json`):

  exact              int/label fields — ``np.array_equal`` (bit-exact required).
  bitwise            bit-identical fp: exact uint8-view compare of the raw bytes.
  exact_set          row-sets compared order-independently (lexsort both sides),
                     for critical-point coordinate lists whose enumeration order
                     is implementation-specific.
  atol               float fields — ``max|candidate - reference| <= atol`` over
                     matching-finite positions, atol never widened past 1e-3.
  order_robust_atol  float rows keyed by ``key_cols`` (row-sort by those columns,
                     then atol), for eigenvector/derivative rows emitted in any
                     order.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

import numpy as np

# BINDING (006 / 009 / 011): the float acceptance tolerance is NEVER widened past
# this. A larger drift is a real op divergence to SURFACE, not to tolerate. Any
# manifest that declares an `atol` above this ceiling is rejected outright.
ATOL_CEILING = 1e-3

# atol-bearing gates and the full gate vocabulary.
_ATOL_GATES = frozenset({"atol", "order_robust_atol"})
GATE_TYPES = frozenset({"exact", "bitwise", "exact_set"} | _ATOL_GATES)

# A scaffold manifest (emitted by make_kernel_scaffold) carries this status and
# MUST hard-fail: an awaiting-source kernel can never produce a green.
SCAFFOLD_STATUS = "AWAITING_PACT_KERNEL_SOURCE"


class GateSpecError(Exception):
    """The gate manifest is invalid / a scaffold / drifted from the reference.

    Raising this is a STRUCTURAL refusal (exit 2): the run could not be honestly
    evaluated, so it fails loud rather than passing by default.
    """


# --------------------------------------------------------------------------- #
# Manifest schema + validation
# --------------------------------------------------------------------------- #
def load_gates(path: str | Path) -> dict[str, Any]:
    """Load and validate a `<k>_gates.json` manifest. Raises GateSpecError."""
    p = Path(path)
    if not p.is_file():
        raise GateSpecError(f"gate manifest not found: {p}")
    try:
        raw = json.loads(p.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise GateSpecError(f"gate manifest unreadable ({p}): {exc}") from exc
    validate_gates(raw)
    return raw


def _is_dtype(name: object) -> bool:
    if not isinstance(name, str):
        return False
    try:
        np.dtype(name)
    except TypeError:
        return False
    return True


def _valid_shape(shape: object) -> bool:
    if not isinstance(shape, list) or not shape:
        return False
    for dim in shape:
        if dim is None:
            continue
        if not isinstance(dim, int) or isinstance(dim, bool) or dim < 0:
            return False
    return True


def validate_gates(gates: object) -> None:
    """Assert a manifest conforms to the schema. Raises GateSpecError otherwise.

    This is where the "never pass by default" and "never widen atol" invariants
    are structurally enforced, before a single array is compared.
    """
    if not isinstance(gates, dict):
        raise GateSpecError("gate manifest must be a JSON object")
    status = gates.get("status")
    if status == SCAFFOLD_STATUS:
        raise GateSpecError(
            f"gate manifest is a scaffold (status={status!r}): "
            "NOT IMPLEMENTED — awaiting pact kernel source. Refusing to evaluate."
        )
    outputs = gates.get("outputs")
    if not isinstance(outputs, dict) or not outputs:
        raise GateSpecError(
            "gate manifest must declare a non-empty 'outputs' object "
            "(a manifest with no gates can never be honestly PASSED)"
        )
    for name, spec in outputs.items():
        where = f"outputs[{name!r}]"
        if not isinstance(spec, dict):
            raise GateSpecError(f"{where} must be an object")
        gate = spec.get("gate")
        if gate not in GATE_TYPES:
            raise GateSpecError(
                f"{where}.gate={gate!r} is not one of {sorted(GATE_TYPES)}"
            )
        if not _is_dtype(spec.get("dtype")):
            raise GateSpecError(
                f"{where}.dtype={spec.get('dtype')!r} is not a valid numpy dtype"
            )
        if not _valid_shape(spec.get("shape")):
            raise GateSpecError(
                f"{where}.shape={spec.get('shape')!r} must be a non-empty list of "
                "non-negative ints and/or null (reference-defined) dims"
            )
        if gate in _ATOL_GATES:
            atol = spec.get("atol")
            if not isinstance(atol, (int, float)) or isinstance(atol, bool):
                raise GateSpecError(f"{where}.atol is required for gate {gate!r}")
            if not (atol > 0):
                raise GateSpecError(f"{where}.atol={atol!r} must be > 0")
            if atol > ATOL_CEILING:
                raise GateSpecError(
                    f"{where}.atol={atol!r} exceeds ATOL_CEILING={ATOL_CEILING} — "
                    "tolerance may NEVER be widened past 1e-3 (surface the "
                    "divergence instead)"
                )
        if gate == "order_robust_atol":
            key_cols = spec.get("key_cols")
            if not isinstance(key_cols, list) or not key_cols or not all(
                isinstance(c, int) and not isinstance(c, bool) and c >= 0
                for c in key_cols
            ):
                raise GateSpecError(
                    f"{where}.key_cols must be a non-empty list of column indices "
                    "for gate 'order_robust_atol'"
                )


# --------------------------------------------------------------------------- #
# Verdict model
# --------------------------------------------------------------------------- #
class ArrayResult:
    __slots__ = ("name", "gate", "ok", "detail")

    def __init__(self, name: str, gate: str, ok: bool, detail: str) -> None:
        self.name = name
        self.gate = gate
        self.ok = ok
        self.detail = detail

    def line(self) -> str:
        tag = "PASS" if self.ok else "FAIL"
        return f"  {tag} {self.name:22s} {self.gate:18s} {self.detail}"


class Verdict:
    def __init__(self) -> None:
        self.results: list[ArrayResult] = []
        self.errors: list[str] = []

    def add(self, name: str, gate: str, ok: bool, detail: str) -> None:
        self.results.append(ArrayResult(name, gate, ok, detail))

    def error(self, msg: str) -> None:
        self.errors.append(msg)

    @property
    def ok(self) -> bool:
        return not self.errors and all(r.ok for r in self.results)

    def report(self) -> str:
        lines = [r.line() for r in self.results]
        for e in self.errors:
            lines.append(f"  FAIL {'<engine>':22s} {'structural':18s} {e}")
        lines.append(f"PARITY: {'PASS' if self.ok else 'FAIL'}")
        return "\n".join(lines)


# --------------------------------------------------------------------------- #
# Shape / dtype conformance
# --------------------------------------------------------------------------- #
def _shape_conforms(actual: tuple[int, ...], spec: list) -> bool:
    """True if `actual` matches `spec` (ints are fixed, None accepts any dim)."""
    if len(actual) != len(spec):
        return False
    return all(dim is None or dim == actual[i] for i, dim in enumerate(spec))


def _shape_matches(
    actual: tuple[int, ...], spec: list, ref: tuple[int, ...]
) -> bool:
    """Candidate shape: fixed dims match the manifest, None dims match the reference."""
    if len(actual) != len(spec):
        return False
    for i, dim in enumerate(spec):
        expected = ref[i] if dim is None else dim
        if actual[i] != expected:
            return False
    return True


# --------------------------------------------------------------------------- #
# Gate evaluators
# --------------------------------------------------------------------------- #
def _rowsort(a: np.ndarray) -> np.ndarray:
    if a.size == 0:
        return a
    return a[np.lexsort(a.T[::-1])]


def _rowsort_by(a: np.ndarray, key_cols: list[int]) -> np.ndarray:
    if a.size == 0:
        return a
    keys = tuple(a[:, c] for c in reversed(key_cols))
    return a[np.lexsort(keys)]


def _finite_mask_mismatch(a: np.ndarray, b: np.ndarray) -> str | None:
    """Return a description if NaN / +Inf / -Inf masks differ, else None.

    Matching non-finite positions (NaN==NaN, +Inf==+Inf, -Inf==-Inf) are allowed;
    any disagreement is a loud FAIL — a candidate must not silently turn a finite
    reference value into NaN/Inf or vice-versa.
    """
    nan_a, nan_b = np.isnan(a), np.isnan(b)
    if not np.array_equal(nan_a, nan_b):
        return f"NaN mask mismatch (cand nan={int(nan_a.sum())} ref nan={int(nan_b.sum())})"
    pinf_a, pinf_b = np.isposinf(a), np.isposinf(b)
    if not np.array_equal(pinf_a, pinf_b):
        return f"+Inf mask mismatch (cand={int(pinf_a.sum())} ref={int(pinf_b.sum())})"
    ninf_a, ninf_b = np.isneginf(a), np.isneginf(b)
    if not np.array_equal(ninf_a, ninf_b):
        return f"-Inf mask mismatch (cand={int(ninf_a.sum())} ref={int(ninf_b.sum())})"
    return None


def _atol_detail(a: np.ndarray, b: np.ndarray, atol: float) -> tuple[bool, str]:
    fa, fb = a.astype(np.float64), b.astype(np.float64)
    mismatch = _finite_mask_mismatch(fa, fb)
    if mismatch is not None:
        return False, mismatch
    finite = np.isfinite(fa)  # identical to isfinite(fb) once masks match
    if finite.any():
        d = float(np.max(np.abs(fa[finite] - fb[finite])))
    else:
        d = 0.0
    return d <= atol, f"max|d|={d:.3e} (atol {atol:.0e})"


def _evaluate_array(
    name: str, spec: dict, cand: np.ndarray, ref: np.ndarray, verdict: Verdict
) -> None:
    gate = spec["gate"]
    want_dtype = np.dtype(spec["dtype"])
    shape_spec = spec["shape"]

    # Reference must itself conform to the manifest — otherwise the manifest has
    # drifted from the authority it claims to gate (structural FAIL, not a skip).
    if ref.dtype != want_dtype:
        verdict.error(
            f"{name}: reference dtype {ref.dtype} != manifest dtype {want_dtype} "
            "(manifest/reference drift)"
        )
        return
    if not _shape_conforms(ref.shape, shape_spec):
        verdict.error(
            f"{name}: reference shape {ref.shape} does not conform to manifest "
            f"shape {shape_spec} (manifest/reference drift)"
        )
        return

    # Candidate dtype + shape are hard fail-loud gates.
    if cand.dtype != want_dtype:
        verdict.add(name, gate, False, f"dtype {cand.dtype} != {want_dtype}")
        return
    if not _shape_matches(cand.shape, shape_spec, ref.shape):
        verdict.add(
            name, gate, False, f"shape {cand.shape} != expected (spec {shape_spec}, ref {ref.shape})"
        )
        return

    if gate == "exact":
        same = bool(np.array_equal(cand, ref))
        mism = int(np.count_nonzero(cand != ref)) if not same else 0
        verdict.add(name, gate, same, f"exact (mismatch={mism})")
    elif gate == "bitwise":
        cb, rb = cand.view(np.uint8).ravel(), ref.view(np.uint8).ravel()
        same = bool(np.array_equal(cb, rb))
        mism = int(np.count_nonzero(cb != rb)) if not same else 0
        verdict.add(name, gate, same, f"bit-identical (byte mismatch={mism})")
    elif gate == "exact_set":
        same = bool(np.array_equal(_rowsort(cand), _rowsort(ref)))
        verdict.add(
            name, gate, same, f"order-robust set (cand n={len(cand)} ref n={len(ref)})"
        )
    elif gate == "atol":
        ok, detail = _atol_detail(cand, ref, float(spec["atol"]))
        verdict.add(name, gate, ok, detail)
    elif gate == "order_robust_atol":
        key_cols = list(spec["key_cols"])
        sa, sb = _rowsort_by(cand, key_cols), _rowsort_by(ref, key_cols)
        ok, detail = _atol_detail(sa, sb, float(spec["atol"]))
        verdict.add(name, gate, ok, f"{detail} order-robust key_cols={key_cols}")
    else:  # pragma: no cover - validate_gates already rejects unknown gates
        verdict.error(f"{name}: unevaluable gate {gate!r}")


# --------------------------------------------------------------------------- #
# Top-level engine
# --------------------------------------------------------------------------- #
def check_parity(
    candidate_path: str | Path,
    reference_path: str | Path,
    gates: dict[str, Any] | str | Path,
) -> Verdict:
    """Produce a strict Verdict for (candidate, reference) under `gates`.

    `gates` may be a pre-loaded manifest dict or a path to a `<k>_gates.json`.
    """
    verdict = Verdict()

    if isinstance(gates, (str, Path)):
        manifest = load_gates(gates)  # validates; raises GateSpecError on refusal
    else:
        validate_gates(gates)
        manifest = gates
    outputs: dict[str, Any] = manifest["outputs"]

    cand = _load_npz(candidate_path, "candidate", verdict)
    ref = _load_npz(reference_path, "reference", verdict)
    if cand is None or ref is None:
        return verdict

    if not cand.files:
        verdict.error("candidate npz contains no arrays (empty/zero-size candidate)")
        return verdict

    manifest_keys = set(outputs)
    ref_keys = set(ref.files)
    cand_keys = set(cand.files)

    # Manifest must cover exactly the reference authority — no gate may be missing.
    for k in sorted(ref_keys - manifest_keys):
        verdict.error(f"reference key {k!r} has no gate in the manifest")
    for k in sorted(manifest_keys - ref_keys):
        verdict.error(f"manifest gate {k!r} is absent from the reference")

    # Candidate keyset must match the manifest exactly.
    for k in sorted(manifest_keys - cand_keys):
        verdict.add(k, str(outputs[k].get("gate", "?")), False, "MISSING in candidate")
    for k in sorted(cand_keys - manifest_keys):
        verdict.error(f"candidate has unexpected extra array {k!r} (not in manifest)")

    for name in sorted(manifest_keys & ref_keys & cand_keys):
        _evaluate_array(name, outputs[name], cand[name], ref[name], verdict)

    return verdict


def _load_npz(path: str | Path, role: str, verdict: Verdict):
    p = Path(path)
    if not p.is_file():
        verdict.error(f"{role} npz not found: {p}")
        return None
    try:
        return np.load(p, allow_pickle=False)
    except Exception as exc:  # noqa: BLE001 - any load failure is a structural FAIL
        verdict.error(f"{role} npz unreadable ({p}): {exc}")
        return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Strict, fail-loud parity oracle for the pact witness kernel suite."
    )
    parser.add_argument("candidate", type=Path, help="candidate_outputs.npz (Molt run)")
    parser.add_argument("reference", type=Path, help="the numpy-fp32 reference .npz")
    parser.add_argument("gates", type=Path, help="the <k>_gates.json manifest")
    args = parser.parse_args(argv)

    try:
        manifest = load_gates(args.gates)
    except GateSpecError as exc:
        print(f"  FAIL {'<manifest>':22s} {'structural':18s} {exc}")
        print("PARITY: FAIL")
        return 2

    verdict = check_parity(args.candidate, args.reference, manifest)
    print(verdict.report())
    if verdict.errors:
        return 2
    return 0 if verdict.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
