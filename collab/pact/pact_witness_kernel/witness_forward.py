"""pact witness FORWARD kernel (Kernel B) — the generator we want molt to run in WASM.

This is the GENERIC witness-generator algorithm (rule-118 FREE: the forward-pass code is not
video-derived; only the learned weights are counted/private). Faithful self-contained extract of
pact's canonical numpy reference:

    POINTER (fidelity oracle): src/tac/boundary_math/lever_b_levelset_generator.py
        - curvelet_directional_B   (the deterministic curvelet/shearlet frequency bank)
        - curvelet_feats           ([sin, cos] of 2*pi * coords @ B)
        - wire_activation/hosc_activation  (non-spectral-bias activations)
        - numpy_levelset_forward   (FiLM-modulated MLP -> K SDF logits)
        - levelset_argmax          (argmax_k phi_k -> the (H,W) partition = lstar)

`verify_against_tac.py` PROVES this extract is bit-identical to that module (the NO-FAKE check).
The de-confound witness adds two ADDITIVE feature extensions (self-orient directional feats +
chroma); both only add MORE columns of the SAME ops (matmul + sin/cos), so this base curvelet
forward exercises the full WASM op set. Pointer for those: experiments/train_levelset_witness_
realized_through_R_mlx.py.

Pipeline:  coords (P,2) + B -> curvelet_feats (P, in_feat) -> FiLM-MLP(code) -> SDF (P,K)
           -> argmax -> lstar (H,W).  Then field_solve(lstar) (Kernel A) -> the viz fields.

ops exercised: matmul, sin, cos, tanh, exp, maximum, argmax, reshape, stack  (all pure numpy).
deterministic: no RNG, no time, no I/O.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np


@dataclass(frozen=True)
class CurveletBankConfig:
    n_scales: int = 4
    n_orient0: int = 6
    f0: float = 2.0
    base: float = 2.0
    n_iso: int = 4


@dataclass(frozen=True)
class LevelSetConfig:
    num_pairs: int = 2
    bank: CurveletBankConfig = field(default_factory=CurveletBankConfig)
    hidden_dim: int = 96
    n_hidden: int = 4
    mod_dim: int = 32
    n_classes: int = 5
    activation: str = (
        "hosc"  # de-confound witness uses hosc; "wire"|"relu" also supported
    )
    wire_w0: float = 20.0
    wire_s0: float = 10.0
    hosc_beta: float = 4.0
    hosc_omega: float = 1.0
    front_end: str = "curvelet"  # "curvelet" | "isotropic"
    iso_n_fourier: int = 48
    iso_sigma: float = 8.0
    max_freq: float | None = None

    def build_B(self) -> np.ndarray:
        if self.front_end == "isotropic":
            return isotropic_fourier_B(self.iso_n_fourier, self.iso_sigma)
        return curvelet_directional_B(self.bank, max_freq=self.max_freq)


def curvelet_directional_B(
    cfg: CurveletBankConfig, max_freq: float | None = None
) -> np.ndarray:
    """(2, n_feats) generic curvelet frequency matrix: J scales x L_j orientations + n_iso coarse."""
    cols: list[np.ndarray] = []
    for j in range(int(cfg.n_scales)):
        f_j = float(cfg.f0) * (float(cfg.base) ** j)
        l_j = int(cfg.n_orient0) * (2 ** (j // 2))  # parabolic curvelet doubling
        for l in range(l_j):
            theta = np.pi * l / l_j
            cols.append(
                np.array([f_j * np.cos(theta), f_j * np.sin(theta)], dtype=np.float32)
            )
    for i in range(int(cfg.n_iso)):
        theta = np.pi * i / max(int(cfg.n_iso), 1)
        f_low = float(cfg.f0) * 0.5
        cols.append(
            np.array([f_low * np.cos(theta), f_low * np.sin(theta)], dtype=np.float32)
        )
    stacked = np.stack(cols, axis=1).astype(np.float32)
    if max_freq is not None:
        norms = np.sqrt((stacked.astype(np.float64) ** 2).sum(axis=0))
        keep = norms <= float(max_freq) + 1e-6
        if not keep.any():
            keep = norms <= float(norms.min()) + 1e-6
        stacked = stacked[:, keep]
    return stacked


def isotropic_fourier_B(n_fourier: int, sigma: float, seed: int = 0) -> np.ndarray:
    rng = np.random.default_rng(seed)
    return (rng.standard_normal((2, n_fourier)) * sigma).astype(np.float32)


def curvelet_feats(coords: np.ndarray, B: np.ndarray) -> np.ndarray:
    """[sin(2*pi X@B), cos(2*pi X@B)] -> (P, 2*n_feats). Identical at train + inflate."""
    with np.errstate(all="ignore"):
        proj = (2.0 * np.pi) * (
            np.asarray(coords, np.float64) @ np.asarray(B, np.float64)
        )
        return np.concatenate([np.sin(proj), np.cos(proj)], axis=-1).astype(np.float32)


def _iso_feats(coords: np.ndarray, B: np.ndarray) -> np.ndarray:
    proj = np.asarray(coords, np.float64) @ np.asarray(B, np.float64)
    return np.concatenate([np.sin(proj), np.cos(proj)], axis=-1).astype(np.float32)


def wire_activation(u: np.ndarray, w0: float = 20.0, s0: float = 10.0) -> np.ndarray:
    u = np.asarray(u, np.float64)
    return (np.cos(w0 * u) * np.exp(-((s0 * u) ** 2))).astype(np.float32)


def hosc_activation(u: np.ndarray, beta: float = 4.0, omega: float = 1.0) -> np.ndarray:
    u = np.asarray(u, np.float64)
    return np.tanh(beta * np.sin(omega * u)).astype(np.float32)


def _act(u: np.ndarray, name: str, **kw) -> np.ndarray:
    if name == "wire":
        return wire_activation(u, kw.get("w0", 20.0), kw.get("s0", 10.0))
    if name == "hosc":
        return hosc_activation(u, kw.get("beta", 4.0), kw.get("omega", 1.0))
    return np.maximum(u, 0.0).astype(np.float32)


def numpy_levelset_forward(
    params: dict, feats: np.ndarray, mod_vec: np.ndarray, cfg: LevelSetConfig
) -> np.ndarray:
    """Pure-numpy mirror of the MLX level-set witness (float64). Returns (P, n_classes) SDF.

    FiLM-per-(pair,frame) modulates each hidden layer (scale,shift). Output head is LINEAR
    (SDFs unbounded; argmax invariant to an output affine).
    """
    p = {k: np.asarray(v, np.float64) for k, v in params.items()}
    feats = np.asarray(feats, np.float64)
    mod_vec = np.asarray(mod_vec, np.float64)
    akw = dict(w0=cfg.wire_w0, s0=cfg.wire_s0, beta=cfg.hosc_beta, omega=cfg.hosc_omega)
    with np.errstate(over="ignore", invalid="ignore", divide="ignore"):
        h = _act(
            feats @ p["in_proj.weight"].T + p["in_proj.bias"], cfg.activation, **akw
        )
        film = (mod_vec @ p["film.weight"].T + p["film.bias"]).reshape(
            cfg.n_hidden, 2, cfg.hidden_dim
        )
        for li in range(cfg.n_hidden):
            scale = 1.0 + film[li, 0]
            shift = film[li, 1]
            h = _act(
                (h @ p[f"hidden.{li}.weight"].T + p[f"hidden.{li}.bias"]) * scale
                + shift,
                cfg.activation,
                **akw,
            )
        return (h @ p["out.weight"].T + p["out.bias"]).astype(np.float32)


def levelset_argmax(
    params: dict, cfg: LevelSetConfig, coords: np.ndarray, pair_idx: int, h: int, w: int
) -> np.ndarray:
    """Per-(pair,frame) generator partition (H,W) uint8 = argmax_k phi_k (numpy-portable)."""
    B = cfg.build_B()
    feats = (
        curvelet_feats(coords, B)
        if cfg.front_end == "curvelet"
        else _iso_feats(coords, B)
    )
    sdf = numpy_levelset_forward(
        params, feats, np.asarray(params["code"])[pair_idx], cfg
    )
    return sdf.argmax(axis=-1).reshape(h, w).astype(np.uint8)


def coord_grid(h: int, w: int) -> np.ndarray:
    """Normalized [-1,1] coordinate grid (P=H*W, 2) in row-major (y,x) order, matching the trainer."""
    ys = np.linspace(-1.0, 1.0, h, dtype=np.float32)
    xs = np.linspace(-1.0, 1.0, w, dtype=np.float32)
    gy, gx = np.meshgrid(ys, xs, indexing="ij")
    return np.stack([gx.ravel(), gy.ravel()], axis=-1).astype(np.float32)


if __name__ == "__main__":
    import sys

    src = sys.argv[1] if len(sys.argv) > 1 else "witness_weights_sample.npz"
    z = np.load(src, allow_pickle=False)
    H = int(z["H"])
    W = int(z["W"])
    cfg = LevelSetConfig(
        num_pairs=int(z["num_pairs"]),
        hidden_dim=int(z["hidden_dim"]),
        n_hidden=int(z["n_hidden"]),
        mod_dim=int(z["mod_dim"]),
        n_classes=int(z["n_classes"]),
        activation=str(z["activation"]),
        front_end=str(z["front_end"]),
    )
    params = {
        k: z[k]
        for k in z.files
        if k.startswith(("in_proj", "hidden", "film", "out", "code"))
    }
    coords = coord_grid(H, W)
    lstar = levelset_argmax(params, cfg, coords, pair_idx=0, h=H, w=W)
    np.savez_compressed("witness_forward_reference.npz", lstar=lstar)
    print(
        f"witness forward -> lstar {lstar.shape} {lstar.dtype}; classes {np.bincount(lstar.ravel(), minlength=cfg.n_classes)}"
    )
