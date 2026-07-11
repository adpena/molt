from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
KERNEL_DIR = ROOT / "collab" / "pact" / "pact_witness_kernel"
if str(KERNEL_DIR) not in sys.path:
    sys.path.insert(0, str(KERNEL_DIR))


def _import(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import {name} from {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _range(array: np.ndarray) -> dict[str, float]:
    return {"min": float(np.min(array)), "max": float(np.max(array))}


def _directed_ulp_stress(logits: np.ndarray, ulps: int) -> int:
    order = np.argsort(logits, axis=1)
    winner = order[:, -1]
    runner = order[:, -2]
    stressed = logits.copy()
    rows = np.arange(logits.shape[0])
    for _ in range(ulps):
        stressed[rows, winner] = np.nextafter(
            stressed[rows, winner], np.float32(-np.inf), dtype=np.float32
        )
        stressed[rows, runner] = np.nextafter(
            stressed[rows, runner], np.float32(np.inf), dtype=np.float32
        )
    return int(np.count_nonzero(np.argmax(stressed, axis=1) != winner))


def measure() -> dict[str, object]:
    fixture = _import("kernel_b_fixture", KERNEL_DIR / "make_weights_fixture.py")
    witness = _import("witness_forward", KERNEL_DIR / "witness_forward.py")
    params = fixture.make()
    config = fixture.CFG
    coords = witness.coord_grid(fixture.H, fixture.W)
    basis = config.build_B()
    features = witness.curvelet_feats(coords, basis)
    p = {key: np.asarray(value, np.float64) for key, value in params.items()}
    hidden_ranges: list[dict[str, object]] = []
    preactivation = (
        features.astype(np.float64) @ p["in_proj.weight"].T + p["in_proj.bias"]
    )
    hidden = witness._act(preactivation, config.activation)
    hidden_ranges.append(
        {
            "layer": "in_proj",
            "preactivation": _range(preactivation),
            "output": _range(hidden),
        }
    )
    film = (p["code"][0] @ p["film.weight"].T + p["film.bias"]).reshape(
        config.n_hidden, 2, config.hidden_dim
    )
    for layer in range(config.n_hidden):
        scale = 1.0 + film[layer, 0]
        shift = film[layer, 1]
        preactivation = (
            hidden @ p[f"hidden.{layer}.weight"].T + p[f"hidden.{layer}.bias"]
        ) * scale + shift
        hidden = witness._act(preactivation, config.activation)
        hidden_ranges.append(
            {
                "layer": f"hidden.{layer}",
                "preactivation": _range(preactivation),
                "output": _range(hidden),
            }
        )
    logits = np.asarray(hidden @ p["out.weight"].T + p["out.bias"], np.float32)
    sorted_logits = np.sort(logits, axis=1)
    margins = sorted_logits[:, -1] - sorted_logits[:, -2]
    top = sorted_logits[:, -1]
    spacing = np.abs(np.spacing(top))
    ulp_margins = np.divide(
        margins,
        spacing,
        out=np.full_like(margins, np.inf, dtype=np.float32),
        where=spacing != 0,
    )
    stress = {
        str(ulps): _directed_ulp_stress(logits, ulps) for ulps in (1, 2, 4, 8, 16)
    }
    return {
        "schema_version": 1,
        "fixture": "synthetic make_weights_fixture.py; real learned weights unavailable",
        "shape": [fixture.H, fixture.W, config.n_classes],
        "activation": config.activation,
        "operations_exercised": [
            "float64 matmul",
            "sin",
            "cos",
            "tanh",
            "float32 cast",
            "argmax",
        ],
        "operations_not_exercised": ["wire activation exp"],
        "feature_range": _range(features),
        "film_range": _range(film),
        "hidden_ranges": hidden_ranges,
        "logit_range": _range(logits),
        "argmax_margin": {
            "minimum": float(np.min(margins)),
            "p001": float(np.quantile(margins, 0.001)),
            "p01": float(np.quantile(margins, 0.01)),
            "median": float(np.median(margins)),
            "minimum_top_logit_ulps": float(np.min(ulp_margins)),
            "zero_margin_pixels": int(np.count_nonzero(margins == 0)),
        },
        "directed_ulp_stress_argmax_changes": stress,
        "verdict": "fallback_not_triggered_on_synthetic_fixture"
        if not any(stress.values())
        else "fallback_triggered_on_synthetic_fixture",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    payload = measure()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
