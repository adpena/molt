"""Parity oracle: compare a candidate field-solve output against the reference.

Usage:
    python check_parity.py candidate_outputs.npz [reference_outputs.npz]

`candidate_outputs.npz` = the molt-WASM run of field_solve() on lstar_sample.npz, saved
with the SAME keys. This declares PASS only if every determinism gate holds.

Gates (see 006_precise_contract.md "Determinism gates"):
  EXACT  : sdf_argmax, boundary, crit_*_rc            (integer/label — bit-exact required)
  ATOL   : sdf_margin_m12, sdf_gap13, dist            (EDT floats; atol 1e-3)
  ATOL   : m_smooth, curvature                        (gaussian_filter floats; atol 1e-3)
  ATOL   : crit_saddle_eigvec                         (eigh, sign-canonicalized; atol 1e-3)
The float atol is deliberately tight: EDT/gaussian on a fixed grid should agree to ~fp32
rounding. A larger drift means the WASM op diverges from scipy and must be investigated.
"""

from __future__ import annotations

from pathlib import Path
import sys

import numpy as np

EXACT = ("sdf_argmax", "boundary")
EXACT_SET = (
    "crit_max_rc",
    "crit_min_rc",
    "crit_saddle_rc",
)  # compare as row-sorted sets
ATOL = {
    "sdf_margin_m12": 1e-3,
    "sdf_gap13": 1e-3,
    "dist": 1e-3,
    "m_smooth": 1e-3,
    "curvature": 1e-3,
    "crit_saddle_eigvec": 1e-3,
}


def _missing_input(path: str, role: str) -> int | None:
    if Path(path).is_file():
        return None
    print(f"  FAIL {role}: missing {path}")
    if path == "reference_outputs.npz":
        print("  regenerate the reference from this directory:")
        print("    python make_fixture.py")
        print("    python field_solve.py lstar_sample.npz")
    elif role == "candidate":
        print("  pass the Molt-produced field_solve output as candidate_outputs.npz")
    return 2


def _rowsort(a: np.ndarray) -> np.ndarray:
    if a.size == 0:
        return a
    return a[np.lexsort(a.T[::-1])]


def main() -> int:
    cand_path = sys.argv[1] if len(sys.argv) > 1 else "candidate_outputs.npz"
    ref_path = sys.argv[2] if len(sys.argv) > 2 else "reference_outputs.npz"
    missing = _missing_input(cand_path, "candidate") or _missing_input(
        ref_path, "reference"
    )
    if missing is not None:
        return missing
    cand, ref = np.load(cand_path), np.load(ref_path)

    ok = True
    for k in ref.files:
        if k not in cand.files:
            print(f"  FAIL {k:20s} MISSING in candidate")
            ok = False
            continue
        a, b = cand[k], ref[k]
        if a.shape != b.shape and k not in EXACT_SET:
            print(f"  FAIL {k:20s} shape {a.shape} != {b.shape}")
            ok = False
            continue
        if k in EXACT:
            same = np.array_equal(a, b)
            print(
                f"  {'PASS' if same else 'FAIL'} {k:20s} exact  (mismatch px={int((a != b).sum())})"
            )
            ok &= same
        elif k in EXACT_SET:
            same = np.array_equal(_rowsort(a), _rowsort(b))
            print(
                f"  {'PASS' if same else 'FAIL'} {k:20s} exact-set  (cand n={len(a)} ref n={len(b)})"
            )
            ok &= same
        elif k == "crit_saddle_eigvec":
            # order-robust: each row is (c, r, dx, dy); (c,r) uniquely identify a saddle, so
            # row-sort both sides by their self-coords before the atol compare. A correct WASM/GPU
            # impl may emit saddles in any order (label enumeration is impl-specific) -> still PASS.
            if a.shape != b.shape:
                print(f"  FAIL {k:20s} count {len(a)} != {len(b)}")
                ok = False
                continue
            sa, sb = _rowsort(a), _rowsort(b)
            d = (
                float(np.max(np.abs(sa.astype(np.float64) - sb.astype(np.float64))))
                if a.size
                else 0.0
            )
            same = d <= ATOL[k]
            print(
                f"  {'PASS' if same else 'FAIL'} {k:20s} max|Δ|={d:.3e}  (atol {ATOL[k]:.0e}, order-robust)"
            )
            ok &= same
        else:
            atol = ATOL.get(k, 1e-3)
            d = (
                float(np.max(np.abs(a.astype(np.float64) - b.astype(np.float64))))
                if a.size
                else 0.0
            )
            same = d <= atol
            print(
                f"  {'PASS' if same else 'FAIL'} {k:20s} max|Δ|={d:.3e}  (atol {atol:.0e})"
            )
            ok &= same

    print("PARITY:", "PASS ✅" if ok else "FAIL ❌")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
