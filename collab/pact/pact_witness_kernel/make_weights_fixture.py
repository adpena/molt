"""Deterministic SYNTHETIC weight fixture for the witness-forward (Kernel B) parity test.

Seeded random weights of the EXACT shapes the witness decoder uses (NOT the real learned
weights — those are private/counted; synthetic is sufficient for numerical WASM parity, same
as the field-solve fixture). Saves witness_weights_sample.npz with config metadata + params.
"""

from __future__ import annotations

import numpy as np

import witness_forward as wf

H, W = 96, 128  # small grid for a fast fixture (P = 12288 coords)
CFG = wf.LevelSetConfig(
    num_pairs=2,
    hidden_dim=96,
    n_hidden=4,
    mod_dim=32,
    n_classes=5,
    activation="hosc",
    front_end="curvelet",
)


def make() -> dict:
    B = CFG.build_B()
    in_feat = 2 * B.shape[1]
    rng = np.random.default_rng(0)  # deterministic
    s = 0.3
    p: dict[str, np.ndarray] = {}
    p["in_proj.weight"] = (rng.standard_normal((CFG.hidden_dim, in_feat)) * s).astype(
        np.float32
    )
    p["in_proj.bias"] = np.zeros(CFG.hidden_dim, np.float32)
    for li in range(CFG.n_hidden):
        p[f"hidden.{li}.weight"] = (
            rng.standard_normal((CFG.hidden_dim, CFG.hidden_dim)) * s
        ).astype(np.float32)
        p[f"hidden.{li}.bias"] = np.zeros(CFG.hidden_dim, np.float32)
    film_out = CFG.n_hidden * 2 * CFG.hidden_dim
    p["film.weight"] = (rng.standard_normal((film_out, CFG.mod_dim)) * 0.05).astype(
        np.float32
    )
    p["film.bias"] = np.zeros(film_out, np.float32)
    p["out.weight"] = (rng.standard_normal((CFG.n_classes, CFG.hidden_dim)) * s).astype(
        np.float32
    )
    p["out.bias"] = np.zeros(CFG.n_classes, np.float32)
    p["code"] = (rng.standard_normal((CFG.num_pairs, CFG.mod_dim)) * 0.5).astype(
        np.float32
    )
    return p


if __name__ == "__main__":
    p = make()
    meta = dict(
        H=H,
        W=W,
        num_pairs=CFG.num_pairs,
        hidden_dim=CFG.hidden_dim,
        n_hidden=CFG.n_hidden,
        mod_dim=CFG.mod_dim,
        n_classes=CFG.n_classes,
        activation=CFG.activation,
        front_end=CFG.front_end,
    )
    np.savez_compressed(
        "witness_weights_sample.npz", **p, **{k: np.array(v) for k, v in meta.items()}
    )
    print(
        "witness_weights_sample.npz  in_feat=%d  params:" % (2 * CFG.build_B().shape[1])
    )
    for k, v in p.items():
        print(f"  {k:18s} {str(v.shape):14s} {v.dtype}")
