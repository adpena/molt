"""pact witness field-solve kernel — the EXACT numpy+scipy compute pact needs molt to run in WASM.

This is the REAL extracted logic from pact's `tools/render_witness_morse_smale_viz.py`
(the Morse-Smale topology viz), made self-contained (no torch, no MLX, no pact imports).
It is the interactive payload of the in-browser witness showcase: given the witness's
per-pixel class map `lstar` (H,W uint8, K=5 comma10k classes), it builds the per-class
signed-distance field and extracts the level-set / Morse-Smale structure that the viz draws.

ALL ops here are pure numpy + scipy.ndimage. This is the kernel we want molt to compile to
WASM (CPU or WebGPU) so the showcase can re-solve on every zoom/scrub, client-side.

scipy.ndimage ops exercised:  distance_transform_edt, gaussian_filter, maximum_filter,
                              minimum_filter, label
numpy ops exercised:          argmax, sort, gradient, percentile, argsort, where, bincount,
                              clip, stack, linalg.eigh, unravel_index-free indexing

Deterministic: no RNG, no time, no I/O. Same input -> same output, bit-for-bit on a given
numpy/scipy build. The committed reference_outputs.npz is the parity oracle.

Contract:
    field_solve(lstar: np.ndarray[H, W] uint8, n_classes: int = 5) -> dict[str, np.ndarray]
"""

from __future__ import annotations

import numpy as np
from scipy import ndimage
from scipy.ndimage import distance_transform_edt, gaussian_filter, label, maximum_filter, minimum_filter

N_CLASSES = 5


def signed_distance_fields(labels: np.ndarray, n_classes: int) -> np.ndarray:
    """Per-class signed distance phi_k (H,W,K): +EDT inside class k, -EDT outside.

    argmax_k phi_k == labels EXACTLY. The ideal SDF representation of the partition.
    """
    a = np.asarray(labels)
    h, w = a.shape
    out = np.zeros((h, w, int(n_classes)), np.float32)
    for k in range(int(n_classes)):
        inside = a == k
        if inside.all():
            out[..., k] = float(max(h, w))
            continue
        if not inside.any():
            out[..., k] = -float(max(h, w))
            continue
        d_in = distance_transform_edt(inside)
        d_out = distance_transform_edt(~inside)
        out[..., k] = (d_in - d_out).astype(np.float32)
    return out


def _sdf_top_fields(lstar: np.ndarray, n_classes: int):
    """From L* build per-class signed distance; return (argmax, m12=top1-top2, gap13=top1-top3)."""
    phi = signed_distance_fields(lstar.astype(np.int64), n_classes)  # (H,W,K)
    srt = np.sort(phi, axis=-1)  # ascending
    top1, top2, top3 = srt[..., -1], srt[..., -2], srt[..., -3]
    am = phi.argmax(-1).astype(np.uint8)
    return am, (top1 - top2).astype(np.float32), (top1 - top3).astype(np.float32)


def _boundary_mask(lab: np.ndarray) -> np.ndarray:
    b = np.zeros(lab.shape, bool)
    b[:-1, :] |= lab[:-1, :] != lab[1:, :]
    b[1:, :] |= lab[:-1, :] != lab[1:, :]
    b[:, :-1] |= lab[:, :-1] != lab[:, 1:]
    b[:, 1:] |= lab[:, :-1] != lab[:, 1:]
    return b


def _critical_points(m_smooth: np.ndarray, gap13: np.ndarray, bnd: np.ndarray):
    """Morse-Smale critical-point taxonomy on the smoothed margin m (>=0).

    minima (index-0): local minima of m on the boundary (deepest boundary pts).
    saddles (index-1): TRIPLE junctions (top3 SDF near-equal) on the boundary; Hessian eigvecs.
    maxima (index-2): local maxima of m (class-cell capitals).
    """
    H, W = m_smooth.shape
    # index-2 maxima: local maxima of m (confident interiors)
    locmax = (m_smooth == maximum_filter(m_smooth, size=15)) & (m_smooth > np.percentile(m_smooth, 90))
    mr, mc = np.where(locmax)
    if mr.size > 40:
        # DETERMINISM GATE (kernel-owned fix): keep the 40 LARGEST m, ties broken by (row, col).
        # lexsort by (col, row, -value) makes the SELECTION depend ONLY on the data — never on the
        # where()/nonzero() enumeration order — so a GPU/unordered-compaction WASM impl picks the
        # same set. (A plain argsort would leave the tie-cut riding on where()'s row-major order;
        # on a real fixture the value at the cut is massively tied, so this matters.)
        vals = m_smooth[mr, mc]
        order = np.lexsort((mc, mr, -vals))[:40]
        mr, mc = mr[order], mc[order]

    # index-0 minima: local minima of m on/near the boundary
    locmin = (m_smooth == minimum_filter(m_smooth, size=11)) & bnd
    nr, nc = np.where(locmin)
    if nr.size > 120:
        # keep the 120 SMALLEST m, ties broken by (row, col) -> enumeration-independent selection.
        vals = m_smooth[nr, nc]
        order = np.lexsort((nc, nr, vals))[:120]
        nr, nc = nr[order], nc[order]

    # index-1 saddles == triple junctions: small top1-top3 gap, on boundary; cluster
    tj = (gap13 < np.percentile(gap13[bnd], 8) if bnd.any() else np.zeros_like(bnd)) & bnd
    lab_tj, n_tj = label(tj)
    sr, sc = [], []
    for i in range(1, n_tj + 1):
        ys, xs = np.where(lab_tj == i)
        sr.append(int(ys.mean()))
        sc.append(int(xs.mean()))
    sr, sc = np.array(sr, int), np.array(sc, int)
    if sr.size:
        # DETERMINISM GATE (kernel-owned fix): canonically order saddles by (row, col) so the
        # `label` component-enumeration order does NOT leak into saddle_rc / saddle_eigvec — both
        # arrays stay row-aligned and impl-independent (a WASM/GPU label with any ordering agrees).
        sord = np.lexsort((sc, sr))
        sr, sc = sr[sord], sc[sord]

    # Hessian eigenvectors at saddles -> separatrix tangents
    eig_segs = []
    if sr.size:
        gy, gx = np.gradient(m_smooth)
        gyy, _ = np.gradient(gy)
        gxy, gxx = np.gradient(gx)
        for r, c in zip(sr, sc):
            r = int(np.clip(r, 1, H - 2))
            c = int(np.clip(c, 1, W - 2))
            Hm = np.array([[gxx[r, c], gxy[r, c]], [gxy[r, c], gyy[r, c]]], float)
            w, v = np.linalg.eigh(Hm)
            vec = v[:, 0]
            # DETERMINISM GATE (kernel-owned fix): eigh's eigenvector SIGN is LAPACK-impl-specific
            # (v and -v are both valid). Canonicalize to first-nonzero-component-positive so the
            # WASM build need NOT match LAPACK's sign convention — only the eigenvalue ordering.
            if vec[0] < 0 or (vec[0] == 0.0 and vec[1] < 0):
                vec = -vec
            eig_segs.append((c, r, float(vec[0]), float(vec[1])))
    return {
        "max_rc": np.stack([mr, mc], 1).astype(np.int32) if mr.size else np.zeros((0, 2), np.int32),
        "min_rc": np.stack([nr, nc], 1).astype(np.int32) if nr.size else np.zeros((0, 2), np.int32),
        "saddle_rc": np.stack([sr, sc], 1).astype(np.int32) if sr.size else np.zeros((0, 2), np.int32),
        "saddle_eigvec": (np.array(eig_segs, np.float32) if eig_segs else np.zeros((0, 4), np.float32)),
    }


def _boundary_curvature(m: np.ndarray, bnd: np.ndarray) -> np.ndarray:
    """|level-set curvature| kappa=div(grad m/|grad m|) of the margin field m, on the separatrix band."""
    ms = gaussian_filter(np.asarray(m, np.float64), sigma=1.5)
    my, mx = np.gradient(ms)
    myy, myx = np.gradient(my)
    mxy, mxx = np.gradient(mx)
    denom = (mx * mx + my * my) ** 1.5 + 1e-6
    kappa = np.abs((mxx * my * my - 2 * mx * my * mxy + myy * mx * mx) / denom)
    kappa = np.clip(kappa, 0, np.percentile(kappa, 99.5))
    out = np.zeros_like(kappa, np.float32)
    out[bnd] = kappa[bnd]
    return out


def _signed_dist_to_boundary(lstar: np.ndarray) -> np.ndarray:
    """Distance (px) from each pixel to the nearest inter-class boundary (the level-set 'height')."""
    bnd = _boundary_mask(lstar)
    if not bnd.any():
        return np.zeros(lstar.shape, np.float32)
    return ndimage.distance_transform_edt(~bnd).astype(np.float32)


def field_solve(lstar: np.ndarray, n_classes: int = N_CLASSES) -> dict[str, np.ndarray]:
    """THE kernel. lstar (H,W uint8 class map) -> the viz field dict (all numpy arrays).

    Faithful to tools/render_witness_morse_smale_viz.py: m_smooth = gaussian_filter(m12, sigma=2.0)
    before the Morse-Smale critical-point extraction; curvature uses internal sigma=1.5.
    """
    lstar = np.asarray(lstar)
    am, m12, gap13 = _sdf_top_fields(lstar, n_classes)
    bnd = _boundary_mask(lstar)
    m_smooth = gaussian_filter(m12, sigma=2.0)
    crit = _critical_points(m_smooth, gap13, bnd)
    curv = _boundary_curvature(m12, bnd)
    dist = _signed_dist_to_boundary(lstar)
    return {
        "sdf_argmax": am,                                  # (H,W) uint8 — MUST equal lstar
        "sdf_margin_m12": m12,                             # (H,W) f32  — top1-top2 SDF margin
        "sdf_gap13": gap13,                                # (H,W) f32  — top1-top3 (triple-junction proximity)
        "boundary": bnd.astype(np.uint8),                  # (H,W) uint8 — inter-class boundary mask
        "m_smooth": m_smooth.astype(np.float32),           # (H,W) f32  — gaussian_filter(m12, 2.0)
        "crit_max_rc": crit["max_rc"],                     # (<=40,2) int32 — index-2 maxima
        "crit_min_rc": crit["min_rc"],                     # (<=120,2) int32 — index-0 minima on boundary
        "crit_saddle_rc": crit["saddle_rc"],               # (k,2) int32 — triple junctions
        "crit_saddle_eigvec": crit["saddle_eigvec"],       # (k,4) f32 (x,y,dx,dy) — separatrix tangents
        "curvature": curv,                                 # (H,W) f32 — |level-set curvature| on boundary
        "dist": dist,                                      # (H,W) f32 — EDT to nearest boundary
    }


if __name__ == "__main__":
    import sys

    src = sys.argv[1] if len(sys.argv) > 1 else "lstar_sample.npz"
    z = np.load(src)
    out = field_solve(z["lstar"])
    np.savez_compressed("reference_outputs.npz", **out)
    print(f"field_solve({src}) -> reference_outputs.npz")
    for k, v in out.items():
        print(f"  {k:20s} {str(v.shape):14s} {v.dtype}")
    # parity self-check: argmax(phi) must equal the input partition
    assert np.array_equal(out["sdf_argmax"], z["lstar"].astype(np.uint8)), "SDF argmax != lstar"
    print("self-check OK: argmax(phi) == lstar")
