#!/usr/bin/env python3
"""First-divergence microscope for the pact witness Kernel A parity endgame.

Turns a future ``check_parity`` FAIL from a week of spelunking into a one-hour
fix: it runs the ``field_solve`` pipeline STAGE BY STAGE, persisting every
intermediate (the kernel is conveniently staged), then localizes the FIRST
diverging stage between a candidate and a reference — with element indices, an
ulp-distance histogram, and an accumulation-order-vs-algorithmic classification.

Subcommands
-----------
run      Execute the staged pipeline on a fixture, persist every intermediate
         stage to a ``stages.npz`` (optionally also the 11 final outputs for
         ``check_parity``). ``--perturb STAGE=ULPS[@i,j]`` injects a synthetic
         k-ulp divergence AFTER a stage is computed (deterministic seed) — the
         teeth-test hook and the libm-drift stress vehicle.
compare  Given two stages.npz files, report the FIRST diverging stage in
         pipeline order, the producing op, indices, ulp histogram, and a
         divergence classification. Exit 1 on divergence.
final    Given only final outputs (the wasm ``candidate_outputs.npz`` case, no
         intermediates), map every diverging output key onto the pipeline DAG
         and report the earliest frontier stage that can have introduced it.
margins  The feasibility certificate: measure how far every float value sits
         from every decision threshold the exact/exact_set gates depend on
         (percentile cuts, local-extremum ties, the top-40/120 keep-cuts, eigh
         eigenvalue gaps). This quantifies how many ulps of upstream drift the
         integer gates provably tolerate.

The staged pipeline NEVER forks from the kernel: it calls ``field_solve``'s own
helpers where granularity allows and mirrors the rest line-for-line;
``tests/tools/test_parity_microscope.py`` gates that its 11 final outputs are
bit-identical to ``field_solve()`` on the real fixture (drift gate with teeth).

Works natively (fast iteration; no molt build needed) and against a wasm
``candidate_outputs.npz`` (``final`` mode).
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import platform
import sys
from pathlib import Path
from typing import Any, Callable

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
KERNEL_DIR = ROOT / "collab" / "pact" / "pact_witness_kernel"
GATES_JSON = KERNEL_DIR / "field_solve_gates.json"
META_KEY = "__parity_microscope_meta__"


def _import_by_path(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot import {name} from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _field_solve_module():
    return _import_by_path("pact_field_solve", KERNEL_DIR / "field_solve.py")


def _parity_engine_module():
    return _import_by_path(
        "pact_parity_engine", ROOT / "collab" / "pact" / "parity" / "check_parity.py"
    )


# --------------------------------------------------------------------------- #
# Stage registry: pipeline order IS first-divergence order.
# op = the numerics that produce the stage; hazard = its cross-stack risk class.
# --------------------------------------------------------------------------- #
STAGES: list[tuple[str, str, str]] = [
    ("lstar", "input fixture (uint8 class map)", "exact"),
    ("phi", "distance_transform_edt (+/- per class)", "exact-safe"),
    ("sdf_argmax", "argmax(phi, axis=-1)", "exact-safe"),
    ("m12", "sort(phi)[-1] - sort(phi)[-2]", "exact-safe"),
    ("gap13", "sort(phi)[-1] - sort(phi)[-3]", "exact-safe"),
    ("boundary", "integer 4-neighbor label compare", "exact-safe"),
    ("w_gauss_s2", "np.exp gaussian weights sigma=2.0", "LIBM-HAZARD"),
    ("m_smooth", "correlate1d x2 (reflect) on m12", "accumulation"),
    ("thr_max", "percentile(m_smooth, 90)", "derived"),
    ("locmax_mask", "m_smooth == maximum_filter(size=15) & > thr", "tie-sensitive"),
    ("crit_max_rc", "lexsort(-val,row,col) keep-40 cut", "cut-sensitive"),
    ("locmin_mask", "m_smooth == minimum_filter(size=11) & bnd", "tie-sensitive"),
    ("crit_min_rc", "lexsort(val,row,col) keep-120 cut", "cut-sensitive"),
    ("thr_saddle", "percentile(gap13[bnd], 8)", "derived"),
    ("tj_mask", "gap13 < thr_saddle & bnd", "threshold-sensitive"),
    ("n_tj", "ndimage.label component count", "exact-safe"),
    ("crit_saddle_rc", "per-component int(mean) + lexsort(row,col)", "exact-safe"),
    ("hessian", "np.gradient chain on m_smooth at saddles", "exact-safe"),
    ("eigvals", "np.linalg.eigh eigenvalues (2x2)", "LAPACK-HAZARD"),
    ("crit_saddle_eigvec", "eigh eigvec, sign-canonicalized", "LAPACK-HAZARD"),
    ("w_gauss_s15", "np.exp gaussian weights sigma=1.5", "LIBM-HAZARD"),
    ("ms_curv", "gaussian_filter(m12 as f64, sigma=1.5)", "accumulation"),
    ("kappa_raw", "gradient chain curvature arithmetic", "exact-safe"),
    ("thr_kappa", "percentile(kappa, 99.5)", "derived"),
    ("curvature", "clip(kappa, 0, thr) masked to boundary", "derived"),
    ("dist", "distance_transform_edt(~boundary)", "exact-safe"),
]
STAGE_ORDER = [name for name, _, _ in STAGES]
STAGE_OP = {name: op for name, op, _ in STAGES}
STAGE_HAZARD = {name: hz for name, _, hz in STAGES}

# Final-output key -> (producing stage, observable parent keys in the final dict).
FINAL_DAG: dict[str, tuple[str, list[str]]] = {
    "sdf_argmax": ("phi", []),
    "sdf_margin_m12": ("phi", []),
    "sdf_gap13": ("phi", []),
    "boundary": ("lstar", []),
    "m_smooth": ("w_gauss_s2/correlate1d", ["sdf_margin_m12"]),
    "crit_max_rc": ("locmax/keep-cut", ["m_smooth"]),
    "crit_min_rc": ("locmin/keep-cut", ["m_smooth", "boundary"]),
    "crit_saddle_rc": ("tj threshold/label", ["sdf_gap13", "boundary"]),
    "crit_saddle_eigvec": ("hessian/eigh", ["m_smooth", "crit_saddle_rc"]),
    "curvature": ("w_gauss_s15/curvature arith", ["sdf_margin_m12", "boundary"]),
    "dist": ("edt(~boundary)", ["boundary"]),
}
FINAL_ORDER = list(FINAL_DAG)


# --------------------------------------------------------------------------- #
# ulp arithmetic (monotonic ordered-integer representation of IEEE-754 floats)
# --------------------------------------------------------------------------- #
def _float_ordinals(a: np.ndarray) -> np.ndarray:
    """Map IEEE-754 floats to monotonically ordered int64 'ordinals'.

    ulp distance == |ordinal(a) - ordinal(b)|. Standard lexicographic bit trick:
    positive floats order as their bit patterns; negatives are reflected.
    """
    a = np.asarray(a)
    if a.dtype == np.float32:
        bits = a.view(np.int32).astype(np.int64)
        sign_mask = np.int64(0x80000000)
    elif a.dtype == np.float64:
        bits = a.view(np.int64)
        sign_mask = np.int64(-0x8000000000000000)
    else:
        raise TypeError(f"ulp ordinals need float32/float64, got {a.dtype}")
    negative = bits < 0
    magnitude = np.where(
        negative, bits & ~sign_mask if a.dtype == np.float64 else bits - sign_mask,
        bits,
    )
    return np.where(negative, -magnitude, magnitude)


def ulp_distance(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    """Element-wise ulp distance as float64 (exact below 2**53, huge => approx)."""
    oa = _float_ordinals(a).astype(np.float64)
    ob = _float_ordinals(b).astype(np.float64)
    return np.abs(oa - ob)


def perturb_ulps(
    arr: np.ndarray,
    ulps: int,
    rng: np.random.Generator,
    at: tuple[int, ...] | None = None,
) -> np.ndarray:
    """Shift float elements by +/-`ulps` ulps (random signs; or one element)."""
    a = np.array(arr, copy=True)
    if a.dtype not in (np.float32, np.float64):
        raise TypeError(f"cannot ulp-perturb dtype {a.dtype}")
    if at is None:
        steps = (rng.integers(0, 2, size=a.shape).astype(np.int64) * 2 - 1) * int(
            ulps
        )
    else:
        steps = np.zeros(a.shape, np.int64)
        steps[at] = int(ulps)
    flat = a.ravel()
    step_flat = steps.ravel()
    for _ in range(int(np.max(np.abs(step_flat))) if flat.size else 0):
        remaining = step_flat != 0
        if not remaining.any():
            break
        direction = np.where(
            step_flat > 0, np.inf, np.where(step_flat < 0, -np.inf, 0.0)
        ).astype(flat.dtype)
        flat[remaining] = np.nextafter(flat[remaining], direction[remaining])
        step_flat = step_flat - np.sign(step_flat)
    return flat.reshape(a.shape)


# --------------------------------------------------------------------------- #
# The staged pipeline (mirrors field_solve.py line-for-line; drift-gated by
# tests/tools/test_parity_microscope.py::test_staged_pipeline_matches_kernel).
# --------------------------------------------------------------------------- #
def _gaussian_weights(sigma: float, truncate: float = 4.0) -> np.ndarray:
    """scipy.ndimage._filters._gaussian_kernel1d(order=0), UNreversed.

    Mirrors scipy's weight computation exactly (np.exp on float64 then
    normalize); this is the kernel's only libm-dependent computation.
    """
    radius = int(truncate * float(sigma) + 0.5)
    sigma2 = float(sigma) * float(sigma)
    x = np.arange(-radius, radius + 1)
    phi_x = np.exp(-0.5 / sigma2 * x**2)
    return phi_x / phi_x.sum()


def _correlate_separable(image: np.ndarray, weights: np.ndarray) -> np.ndarray:
    """gaussian_filter's separable correlate1d passes with explicit weights."""
    from scipy.ndimage import correlate1d

    out = correlate1d(image, weights[::-1], axis=0, mode="reflect")
    return correlate1d(out, weights[::-1], axis=1, mode="reflect")


class PerturbSpec:
    """A parsed --perturb STAGE=ULPS[@i,j,...] injection."""

    def __init__(self, stage: str, ulps: int, at: tuple[int, ...] | None) -> None:
        self.stage = stage
        self.ulps = ulps
        self.at = at

    @classmethod
    def parse(cls, raw: str) -> "PerturbSpec":
        stage, _, rest = raw.partition("=")
        if not rest:
            raise SystemExit(f"--perturb wants STAGE=ULPS[@i,j], got {raw!r}")
        ulps_part, _, at_part = rest.partition("@")
        at = tuple(int(t) for t in at_part.split(",")) if at_part else None
        return cls(stage.strip(), int(ulps_part), at)


def staged_field_solve(
    lstar: np.ndarray,
    n_classes: int = 5,
    perturbs: list[PerturbSpec] | None = None,
    perturb_seed: int = 20260711,
) -> tuple[dict[str, np.ndarray], dict[str, np.ndarray]]:
    """Run field_solve stage-by-stage; return (final_outputs, stages).

    `perturbs` inject k-ulp drift into a named stage AFTER it is computed, so
    every downstream consumer sees the perturbed value — a synthetic model of a
    cross-stack (libm/FMA/SIMD) divergence at exactly that op.
    """
    from scipy import ndimage
    from scipy.ndimage import maximum_filter, minimum_filter

    fs = _field_solve_module()
    rng = np.random.default_rng(perturb_seed)
    by_stage: dict[str, list[PerturbSpec]] = {}
    for p in perturbs or []:
        if p.stage not in STAGE_ORDER:
            raise SystemExit(f"--perturb unknown stage {p.stage!r}")
        by_stage.setdefault(p.stage, []).append(p)

    stages: dict[str, np.ndarray] = {}

    def put(name: str, value: np.ndarray) -> np.ndarray:
        value = np.asarray(value)
        for p in by_stage.get(name, []):
            value = perturb_ulps(value, p.ulps, rng, at=p.at)
        stages[name] = value
        return value

    lstar = np.asarray(lstar)
    put("lstar", lstar)

    # --- _sdf_top_fields, staged --------------------------------------------
    phi = put("phi", fs.signed_distance_fields(lstar.astype(np.int64), n_classes))
    srt = np.sort(phi, axis=-1)
    top1, top2, top3 = srt[..., -1], srt[..., -2], srt[..., -3]
    am = put("sdf_argmax", phi.argmax(-1).astype(np.uint8))
    m12 = put("m12", (top1 - top2).astype(np.float32))
    gap13 = put("gap13", (top1 - top3).astype(np.float32))

    bnd = put("boundary", fs._boundary_mask(lstar)).astype(bool)

    # --- m_smooth = gaussian_filter(m12, sigma=2.0), staged ------------------
    w2 = put("w_gauss_s2", _gaussian_weights(2.0))
    m_smooth = put("m_smooth", _correlate_separable(m12, w2))

    # --- _critical_points, staged --------------------------------------------
    H, W = m_smooth.shape
    thr_max = float(put("thr_max", np.float64(np.percentile(m_smooth, 90))))
    locmax = put(
        "locmax_mask",
        (m_smooth == maximum_filter(m_smooth, size=15)) & (m_smooth > thr_max),
    )
    mr, mc = np.where(locmax)
    if mr.size > 40:
        vals = m_smooth[mr, mc]
        order = np.lexsort((mc, mr, -vals))[:40]
        mr, mc = mr[order], mc[order]
    crit_max_rc = put(
        "crit_max_rc",
        np.stack([mr, mc], 1).astype(np.int32)
        if mr.size
        else np.zeros((0, 2), np.int32),
    )

    locmin = put(
        "locmin_mask", (m_smooth == minimum_filter(m_smooth, size=11)) & bnd
    )
    nr, nc = np.where(locmin)
    if nr.size > 120:
        vals = m_smooth[nr, nc]
        order = np.lexsort((nc, nr, vals))[:120]
        nr, nc = nr[order], nc[order]
    crit_min_rc = put(
        "crit_min_rc",
        np.stack([nr, nc], 1).astype(np.int32)
        if nr.size
        else np.zeros((0, 2), np.int32),
    )

    thr_saddle = float(
        put(
            "thr_saddle",
            np.float64(np.percentile(gap13[bnd], 8)) if bnd.any() else np.float64(0),
        )
    )
    tj = put(
        "tj_mask",
        (gap13 < thr_saddle if bnd.any() else np.zeros_like(bnd)) & bnd,
    )
    lab_tj, n_tj = ndimage.label(tj)
    put("n_tj", np.int64(n_tj))
    sr, sc = [], []
    for i in range(1, n_tj + 1):
        ys, xs = np.where(lab_tj == i)
        sr.append(int(ys.mean()))
        sc.append(int(xs.mean()))
    sr, sc = np.array(sr, int), np.array(sc, int)
    if sr.size:
        sord = np.lexsort((sc, sr))
        sr, sc = sr[sord], sc[sord]
    crit_saddle_rc = put(
        "crit_saddle_rc",
        np.stack([sr, sc], 1).astype(np.int32)
        if sr.size
        else np.zeros((0, 2), np.int32),
    )

    eig_segs = []
    hessians = np.zeros((sr.size, 2, 2), np.float64)
    eigvals = np.zeros((sr.size, 2), np.float64)
    if sr.size:
        gy, gx = np.gradient(m_smooth)
        gyy, _ = np.gradient(gy)
        gxy, gxx = np.gradient(gx)
        for idx, (r, c) in enumerate(zip(sr, sc)):
            r = int(np.clip(r, 1, H - 2))
            c = int(np.clip(c, 1, W - 2))
            Hm = np.array(
                [[gxx[r, c], gxy[r, c]], [gxy[r, c], gyy[r, c]]], float
            )
            hessians[idx] = Hm
            w, v = np.linalg.eigh(Hm)
            eigvals[idx] = w
            vec = v[:, 0]
            if vec[0] < 0 or (vec[0] == 0.0 and vec[1] < 0):
                vec = -vec
            eig_segs.append((c, r, float(vec[0]), float(vec[1])))
    put("hessian", hessians)
    put("eigvals", eigvals)
    crit_saddle_eigvec = put(
        "crit_saddle_eigvec",
        np.array(eig_segs, np.float32) if eig_segs else np.zeros((0, 4), np.float32),
    )

    # --- _boundary_curvature, staged ------------------------------------------
    w15 = put("w_gauss_s15", _gaussian_weights(1.5))
    ms = put("ms_curv", _correlate_separable(np.asarray(m12, np.float64), w15))
    my, mx = np.gradient(ms)
    myy, myx = np.gradient(my)
    mxy, mxx = np.gradient(mx)
    denom = (mx * mx + my * my) ** 1.5 + 1e-6
    kappa = put(
        "kappa_raw",
        np.abs((mxx * my * my - 2 * mx * my * mxy + myy * mx * mx) / denom),
    )
    thr_kappa = float(put("thr_kappa", np.float64(np.percentile(kappa, 99.5))))
    kappa_clipped = np.clip(kappa, 0, thr_kappa)
    curv = np.zeros_like(kappa_clipped, np.float32)
    curv[bnd] = kappa_clipped[bnd]
    curv = put("curvature", curv)

    dist = put("dist", fs._signed_dist_to_boundary(np.asarray(stages["lstar"])))

    outputs = {
        "sdf_argmax": am,
        "sdf_margin_m12": m12,
        "sdf_gap13": gap13,
        "boundary": np.asarray(stages["boundary"]).astype(np.uint8),
        "m_smooth": m_smooth.astype(np.float32),
        "crit_max_rc": crit_max_rc,
        "crit_min_rc": crit_min_rc,
        "crit_saddle_rc": crit_saddle_rc,
        "crit_saddle_eigvec": crit_saddle_eigvec,
        "curvature": curv,
        "dist": dist,
    }
    return outputs, stages


def _env_fingerprint() -> dict[str, Any]:
    import scipy

    info: dict[str, Any] = {
        "numpy": np.__version__,
        "scipy": scipy.__version__,
        "python": sys.version.split()[0],
        "platform": platform.platform(),
        "NPY_DISABLE_CPU_FEATURES": os.environ.get("NPY_DISABLE_CPU_FEATURES", ""),
    }
    try:
        cfg = np.show_config(mode="dicts")
        info["blas"] = cfg["Build Dependencies"]["blas"].get("name", "NONE")
        info["lapack"] = cfg["Build Dependencies"]["lapack"].get("name", "NONE")
        info["simd_found"] = cfg.get("SIMD Extensions", {}).get("found", [])
    except Exception:  # pragma: no cover - show_config shape varies
        info["blas"] = info["lapack"] = "unknown"
    return info


# --------------------------------------------------------------------------- #
# compare: first-divergence localization
# --------------------------------------------------------------------------- #
def _describe_divergence(name: str, a: np.ndarray, b: np.ndarray) -> list[str]:
    lines: list[str] = []
    if a.shape != b.shape:
        lines.append(f"    shape {a.shape} != {b.shape} (set/count change)")
        return lines
    if a.dtype != b.dtype:
        lines.append(f"    dtype {a.dtype} != {b.dtype}")
        return lines
    diff_mask = a != b
    both_nan = (
        np.isnan(a) & np.isnan(b)
        if a.dtype in (np.float32, np.float64)
        else np.zeros(a.shape, bool)
    )
    diff_mask = diff_mask & ~both_nan
    n = int(np.count_nonzero(diff_mask))
    total = max(a.size, 1)
    lines.append(f"    {n}/{a.size} elements differ ({100.0 * n / total:.4f}%)")
    idx = np.argwhere(diff_mask)
    show = idx[:10]
    for where in show:
        t = tuple(int(x) for x in where)
        lines.append(f"      at {t}: cand={a[t]!r} ref={b[t]!r}")
    if len(idx) > len(show):
        lines.append(f"      ... {len(idx) - len(show)} more")
    if a.dtype in (np.float32, np.float64) and n:
        d = ulp_distance(a[diff_mask], b[diff_mask])
        hist_edges = [1, 2, 4, 16, 256, 2**20]
        counts = []
        prev = 0
        for edge in hist_edges:
            counts.append(int(np.count_nonzero((d > prev) & (d <= edge))))
            prev = edge
        counts.append(int(np.count_nonzero(d > hist_edges[-1])))
        labels = ["=1", "2", "3-4", "5-16", "17-256", "257-2^20", ">2^20"]
        hist = "  ".join(f"{lab}:{c}" for lab, c in zip(labels, counts) if c)
        lines.append(f"    ulp histogram: {hist}  max={d.max():.3g}")
        frac = n / total
        max_ulp = float(d.max())
        if max_ulp <= 4 and frac < 0.02:
            verdict = (
                "last-ulp drift: accumulation-order / libm-grade divergence "
                "(NOT algorithmic)"
            )
        elif frac < 1e-4:
            verdict = "localized flip(s): likely tie/threshold crossing upstream"
        else:
            verdict = "ALGORITHMIC divergence (different computation, not rounding)"
        lines.append(f"    classification: {verdict}")
    return lines


def cmd_compare(cand_path: Path, ref_path: Path) -> int:
    cand = np.load(cand_path, allow_pickle=False)
    ref = np.load(ref_path, allow_pickle=False)
    first: str | None = None
    divergent: list[str] = []
    report_lines: list[str] = []
    for name in STAGE_ORDER:
        if name not in cand.files or name not in ref.files:
            if (name in cand.files) != (name in ref.files):
                report_lines.append(f"  MISSING {name} on one side")
                divergent.append(name)
                first = first or name
            continue
        a, b = np.atleast_1d(cand[name]), np.atleast_1d(ref[name])
        same = a.shape == b.shape and a.dtype == b.dtype
        if same:
            # bitwise: NaNs equal iff same bit pattern
            same = a.tobytes() == b.tobytes()
        if same:
            continue
        divergent.append(name)
        if first is None:
            first = name
            report_lines.append(
                f"  FIRST DIVERGENCE: stage '{name}'  op: {STAGE_OP[name]}  "
                f"hazard-class: {STAGE_HAZARD[name]}"
            )
            report_lines.extend(_describe_divergence(name, a, b))
    if first is None:
        print("ALL STAGES BIT-IDENTICAL (no divergence)")
        return 0
    print("\n".join(report_lines))
    downstream = [s for s in divergent if s != first]
    if downstream:
        print(f"  downstream stages also diverging: {', '.join(downstream)}")
    print(f"FIRST-DIVERGENT-STAGE: {first}")
    return 1


# --------------------------------------------------------------------------- #
# final: wasm candidate mode (11 output keys, no intermediates)
# --------------------------------------------------------------------------- #
def cmd_final(cand_path: Path, ref_path: Path) -> int:
    cand = np.load(cand_path, allow_pickle=False)
    ref = np.load(ref_path, allow_pickle=False)
    diverging: list[str] = []
    matching: set[str] = set()
    for key in FINAL_ORDER:
        if key not in cand.files or key not in ref.files:
            print(f"  MISSING {key} in {'candidate' if key not in cand.files else 'reference'}")
            diverging.append(key)
            continue
        a, b = cand[key], ref[key]
        if a.shape == b.shape and a.dtype == b.dtype and a.tobytes() == b.tobytes():
            matching.add(key)
            continue
        diverging.append(key)
        stage, parents = FINAL_DAG[key]
        parent_state = (
            "all parents match -> divergence INTRODUCED at this op"
            if all(p in matching for p in parents)
            else f"parents also diverging: "
            f"{[p for p in parents if p not in matching]}"
        )
        print(f"  DIVERGES {key}  (producing op: {stage}; {parent_state})")
        for line in _describe_divergence(key, a, b):
            print(line)
    if not diverging:
        print("ALL FINAL OUTPUTS BIT-IDENTICAL")
        return 0
    frontier = [
        k
        for k in diverging
        if all(p in matching for p in FINAL_DAG.get(k, ("", []))[1])
    ]
    print(f"FRONTIER (earliest ops that introduced divergence): {frontier}")
    # Gate verdict via the shared engine (the actual acceptance authority).
    engine = _parity_engine_module()
    verdict = engine.check_parity(cand_path, ref_path, GATES_JSON)
    print("--- shared-engine gate verdict (field_solve_gates.json) ---")
    print(verdict.report())
    return 0 if verdict.ok else 1


# --------------------------------------------------------------------------- #
# margins: the feasibility certificate
# --------------------------------------------------------------------------- #
def _second_in_window(
    field: np.ndarray, r: int, c: int, size: int
) -> tuple[float, int]:
    """(margin to the largest strictly-smaller value in the window, #exact ties)."""
    h, w = field.shape
    half = size // 2
    win = field[
        max(0, r - half) : min(h, r + half + 1),
        max(0, c - half) : min(w, c + half + 1),
    ]
    center = field[r, c]
    ties = int(np.count_nonzero(win == center)) - 1
    smaller = win[win < center]
    margin = float(center - smaller.max()) if smaller.size else float("inf")
    return margin, ties


def _cut_report(vals: np.ndarray, keep: int, largest: bool) -> list[str]:
    lines: list[str] = []
    n = vals.size
    if n <= keep:
        lines.append(f"    no cut ({n} candidates <= keep {keep})")
        return lines
    ordered = np.sort(vals)[::-1] if largest else np.sort(vals)
    kept_last, dropped_first = ordered[keep - 1], ordered[keep]
    tied = int(np.count_nonzero(vals == kept_last))
    gap = abs(float(kept_last) - float(dropped_first))
    spacing = float(np.spacing(np.abs(kept_last), dtype=vals.dtype))
    lines.append(
        f"    {n} candidates, cut at {keep}: value_at_cut={kept_last!r} "
        f"tied_at_cut={tied} gap_to_next={gap:.6g} "
        f"({gap / spacing if spacing else 0:.0f} ulps)"
    )
    if gap == 0.0:
        lines.append(
            "    cut lands INSIDE an exact-tie group -> survives ONLY the "
            "kernel's (row,col) tie-break; any 1-ulp VALUE split inside the "
            "group changes the selected set"
        )
    return lines


def cmd_margins(stages_path: Path) -> int:
    z = np.load(stages_path, allow_pickle=False)
    m_smooth = z["m_smooth"]
    gap13 = z["gap13"]
    bnd = z["boundary"].astype(bool)
    print("=== exact/exact_set gate robustness margins ===")

    thr_max = float(z["thr_max"])
    d = np.abs(m_smooth.astype(np.float64) - thr_max)
    spacing = float(np.spacing(np.float32(abs(thr_max))))
    print(f"  [crit_max] percentile-90 threshold = {thr_max!r}")
    print(
        f"    nearest value-to-threshold distance: {d.min():.6g} "
        f"({d.min() / spacing:.0f} f32-ulps at threshold); "
        f"within 16 ulps: {int(np.count_nonzero(d <= 16 * spacing))} px"
    )
    locmax = z["locmax_mask"]
    margins = []
    plateau = 0
    for r, c in np.argwhere(locmax):
        m, ties = _second_in_window(m_smooth, int(r), int(c), 15)
        margins.append(m)
        plateau += ties > 0
    if margins:
        finite = [m for m in margins if np.isfinite(m)]
        print(
            f"    local-max pixels: {len(margins)}; window tie-plateaus: "
            f"{plateau}; min margin to next window value: "
            f"{min(finite) if finite else float('inf'):.6g}"
        )
    vals_max = m_smooth[locmax]
    print("    keep-40 cut:")
    for line in _cut_report(vals_max, 40, largest=True):
        print(line)

    locmin = z["locmin_mask"]
    margins_min: list[float] = []
    plateau_min = 0
    for r, c in np.argwhere(locmin):
        m, ties = _second_in_window(-m_smooth, int(r), int(c), 11)
        margins_min.append(m)
        plateau_min += ties > 0
    if margins_min:
        finite = [m for m in margins_min if np.isfinite(m)]
        print(
            f"  [crit_min] local-min pixels: {len(margins_min)}; tie-plateaus: "
            f"{plateau_min}; min margin: "
            f"{min(finite) if finite else float('inf'):.6g}"
        )
    print("    keep-120 cut:")
    for line in _cut_report(m_smooth[locmin], 120, largest=False):
        print(line)

    thr_saddle = float(z["thr_saddle"])
    gb = gap13[bnd].astype(np.float64)
    d_saddle = np.abs(gb - thr_saddle)
    spacing_s = float(np.spacing(np.float32(abs(thr_saddle)))) or 1e-45
    print(
        f"  [crit_saddle] percentile-8 threshold = {thr_saddle!r}; nearest "
        f"boundary gap13 distance: {d_saddle.min():.6g} "
        f"({d_saddle.min() / spacing_s:.0f} f32-ulps) "
        f"[gap13 is EDT-derived: bit-exact cross-stack]"
    )

    eigvals = z["eigvals"]
    if eigvals.size:
        gaps = np.abs(eigvals[:, 1] - eigvals[:, 0])
        norms = np.abs(eigvals).max(axis=1) + 1e-300
        print(
            f"  [saddle_eigvec] eigh 2x2: n={len(eigvals)}; min |l2-l1| = "
            f"{gaps.min():.6g}; min relative gap = {(gaps / norms).min():.6g} "
            f"(eigvec sensitivity ~ ||dH|| / gap; atol gate 1e-3)"
        )
    print("=== interpretation ===")
    print(
        "  exact_set gates survive any upstream drift strictly smaller than\n"
        "  half the SMALLEST margin above, PROVIDED exact-tie plateaus/groups\n"
        "  are computed identically on both stacks (same algorithm+order).\n"
        "  Ties that are IDENTICAL COMPUTATIONS drift together (safe); only\n"
        "  coincidental value ties from different windows can split."
    )
    return 0


# --------------------------------------------------------------------------- #
# run
# --------------------------------------------------------------------------- #
def cmd_run(
    fixture: Path,
    out: Path,
    final_out: Path | None,
    perturbs: list[PerturbSpec],
    perturb_seed: int,
) -> int:
    z = np.load(fixture, allow_pickle=False)
    lstar = z["lstar"]
    outputs, stages = staged_field_solve(
        lstar, perturbs=perturbs, perturb_seed=perturb_seed
    )
    meta = json.dumps(
        {
            "env": _env_fingerprint(),
            "fixture": str(fixture),
            "perturbs": [
                {"stage": p.stage, "ulps": p.ulps, "at": p.at} for p in perturbs
            ],
            "stage_order": STAGE_ORDER,
        }
    )
    save: dict[str, np.ndarray] = {META_KEY: np.frombuffer(meta.encode(), np.uint8)}
    save.update(stages)
    out.parent.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(out, **save)
    print(f"stages -> {out}  ({len(stages)} stages)")
    if final_out is not None:
        np.savez_compressed(final_out, **outputs)
        print(f"final outputs -> {final_out}  ({len(outputs)} keys)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_run = sub.add_parser("run", help="run staged pipeline, persist intermediates")
    p_run.add_argument("--fixture", type=Path, required=True)
    p_run.add_argument("--out", type=Path, required=True)
    p_run.add_argument("--final-out", type=Path, default=None)
    p_run.add_argument(
        "--perturb",
        action="append",
        default=[],
        metavar="STAGE=ULPS[@i,j]",
        help="inject +/-k-ulp drift into a stage (repeatable)",
    )
    p_run.add_argument("--perturb-seed", type=int, default=20260711)

    p_cmp = sub.add_parser("compare", help="first-divergence between stage files")
    p_cmp.add_argument("candidate", type=Path)
    p_cmp.add_argument("reference", type=Path)

    p_fin = sub.add_parser("final", help="final-outputs-only mode (wasm candidate)")
    p_fin.add_argument("candidate", type=Path)
    p_fin.add_argument("reference", type=Path)

    p_mar = sub.add_parser("margins", help="gate robustness margins for a stage file")
    p_mar.add_argument("stages", type=Path)

    args = parser.parse_args(argv)
    if args.cmd == "run":
        return cmd_run(
            args.fixture,
            args.out,
            args.final_out,
            [PerturbSpec.parse(raw) for raw in args.perturb],
            args.perturb_seed,
        )
    if args.cmd == "compare":
        return cmd_compare(args.candidate, args.reference)
    if args.cmd == "final":
        return cmd_final(args.candidate, args.reference)
    if args.cmd == "margins":
        return cmd_margins(args.stages)
    raise SystemExit(f"unknown command {args.cmd!r}")  # pragma: no cover


if __name__ == "__main__":
    raise SystemExit(main())
