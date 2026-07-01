"""Deterministic fixture generator for the pact witness field-solve parity test.

Produces lstar_sample.npz: a (384, 512) uint8 class map with comma10k class indices
  0=Road  1=Lane  2=Undrivable(sky/offroad)  3=Movable(car)  4=MyCar(ego hood)

PURE GEOMETRY — no RNG, no time, no I/O beyond the output. Reproducible byte-for-byte.
It is a *synthetic* road-scene-STRUCTURED partition (NOT a real witness output): its only
job is to exercise the kernel's full op set (multi-class boundaries, the apex triple-junction
where Road/Lane/Undrivable meet, converging-lane curvature, per-class EDT) so the molt-WASM
parity check is meaningful. A real witness-φ argmax bundle follows once pact's build
constraint clears; for *numerical parity* of the compiled kernel the input only needs the
right shape/dtype/structure, which this provides deterministically.
"""

from __future__ import annotations

import numpy as np

H, W = 384, 512


def make_lstar() -> np.ndarray:
    a = np.full((H, W), 2, np.uint8)  # default = Undrivable (sky + offroad)
    yy, xx = np.mgrid[0:H, 0:W].astype(np.float64)

    horizon = 0.42 * H  # apex row (road vanishing point)
    apex_x = 0.52 * W  # apex column (slightly right of center)
    half_at_bottom = 0.46 * W  # road half-width at the bottom of the frame

    # Road = trapezoid converging from the bottom to the apex (perspective).
    t = np.clip((yy - horizon) / (H - horizon), 0.0, 1.0)  # 0 at apex, 1 at bottom
    half_w = t * half_at_bottom
    on_road = (yy >= horizon) & (np.abs(xx - apex_x) <= half_w)
    a[on_road] = 0  # Road

    # Two lane markings: lines from near the apex fanning out to the bottom corners.
    for sgn in (-1.0, +1.0):
        # lane x as a function of row: starts at apex, ends at +/- 0.5*half_at_bottom
        lane_x = apex_x + sgn * t * (0.52 * half_at_bottom)
        width = 1.5 + 4.0 * t  # perspective: thin near apex, thick near car
        lane = on_road & (np.abs(xx - lane_x) <= width)
        a[lane] = 1  # Lane

    # A Movable vehicle: a box sitting on the road, mid-distance.
    cy, cx, ch, cw = int(0.58 * H), int(0.40 * W), int(0.10 * H), int(0.09 * W)
    a[cy : cy + ch, cx : cx + cw] = 3  # Movable

    # Ego hood: bottom band (static MyCar region).
    a[int(0.86 * H) :, :] = 4  # MyCar

    return a


if __name__ == "__main__":
    lstar = make_lstar()
    np.savez_compressed("lstar_sample.npz", lstar=lstar)
    counts = np.bincount(lstar.ravel(), minlength=5)
    names = ["Road", "Lane", "Undrivable", "Movable", "MyCar"]
    print(f"lstar_sample.npz  shape={lstar.shape} dtype={lstar.dtype}")
    for k, (n, c) in enumerate(zip(names, counts)):
        print(f"  class {k} {n:11s} {c:7d}px  ({100 * c / lstar.size:5.2f}%)")
