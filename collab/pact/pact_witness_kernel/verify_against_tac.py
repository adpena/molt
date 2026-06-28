"""NO-FAKE fidelity proof: this extract (witness_forward.py) == canonical tac module, bit-for-bit.

Imports src/tac/boundary_math/lever_b_levelset_generator.py and runs BOTH it and the shipped
extract on the SAME synthetic weights/coords, asserting identical curvelet bank, features,
SDF forward, and argmax partition. Run from a checkout that has `tac` importable:

    PYTHONPATH=/path/to/pact/src python verify_against_tac.py

If this prints ALL-MATCH, the kernel we hand molt is provably the real pact compute, not a
re-derivation. (Pointers: tac lever_b_levelset_generator.curvelet_directional_B / curvelet_feats /
numpy_levelset_forward / levelset_argmax.)
"""

from __future__ import annotations

import numpy as np

import witness_forward as wf
from make_weights_fixture import CFG, H, W, make

try:
    from tac.boundary_math import lever_b_levelset_generator as T
except Exception as e:  # pragma: no cover
    raise SystemExit(f"tac not importable ({e}); run with PYTHONPATH=<pact>/src")


def main() -> int:
    p = make()
    tcfg = T.LevelSetConfig(num_pairs=CFG.num_pairs, hidden_dim=CFG.hidden_dim, n_hidden=CFG.n_hidden,
                            mod_dim=CFG.mod_dim, n_classes=CFG.n_classes, activation=CFG.activation,
                            front_end=CFG.front_end)
    coords = wf.coord_grid(H, W)

    checks = []
    # 1) curvelet bank
    Be, Bt = wf.curvelet_directional_B(wf.CurveletBankConfig()), T.curvelet_directional_B(T.CurveletBankConfig())
    checks.append(("curvelet_directional_B", np.array_equal(Be, Bt)))
    # 2) features
    fe, ft = wf.curvelet_feats(coords, Be), T.curvelet_feats(coords, Bt)
    checks.append(("curvelet_feats", np.array_equal(fe, ft)))
    # 3) forward SDF (use tac's build_B to be apples-to-apples)
    sdf_e = wf.numpy_levelset_forward(p, wf.curvelet_feats(coords, wf.LevelSetConfig().build_B()), p["code"][0], wf.LevelSetConfig(**_cfg_kw()))
    sdf_t = T.numpy_levelset_forward(p, T.curvelet_feats(coords, tcfg.build_B()), p["code"][0], tcfg)
    checks.append(("numpy_levelset_forward", np.array_equal(sdf_e, sdf_t)))
    # 4) argmax partition
    am_e = wf.levelset_argmax(p, wf.LevelSetConfig(**_cfg_kw()), coords, 0, H, W)
    am_t = T.levelset_argmax(p, tcfg, coords, 0, H, W)
    checks.append(("levelset_argmax", np.array_equal(am_e, am_t)))

    ok = True
    for name, same in checks:
        print(f"  {'MATCH' if same else 'DIFFER':6s}  {name}")
        ok &= same
    print("FIDELITY:", "ALL-MATCH ✅ (extract == canonical tac)" if ok else "DIVERGENCE ❌")
    return 0 if ok else 1


def _cfg_kw():
    return dict(num_pairs=CFG.num_pairs, hidden_dim=CFG.hidden_dim, n_hidden=CFG.n_hidden,
                mod_dim=CFG.mod_dim, n_classes=CFG.n_classes, activation=CFG.activation, front_end=CFG.front_end)


if __name__ == "__main__":
    raise SystemExit(main())
