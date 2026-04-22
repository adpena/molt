from __future__ import annotations

import ast
from pathlib import Path

import pytest

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def _flatten_numeric(values):
    out = []
    for value in values:
        if isinstance(value, list):
            out.extend(_flatten_numeric(value))
        else:
            out.append(float(value))
    return out


def test_wasm_linked_tinygrad_random_surface_matches_upstream_samples(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "tinygrad_random_wasm.py"
    src.write_text(
        "from tinygrad import Tensor\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.rand(2, 3).to_list())\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.uniform(2, 3, low=-1.0, high=1.0).to_list())\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.glorot_uniform(2, 3).to_list())\n"
        "Tensor.manual_seed(42)\n"
        "print(Tensor.randn(2, 3).to_list())\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    lines = [ast.literal_eval(line) for line in run.stdout.strip().splitlines()]
    assert _flatten_numeric(lines[0]) == pytest.approx(
        [
            0.9970332384109497,
            0.5899163484573364,
            0.2225480079650879,
            0.7550519704818726,
            0.9056503772735596,
            0.8648829460144043,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[1]) == pytest.approx(
        [
            0.9940664768218994,
            0.17983269691467285,
            -0.5549039840698242,
            0.5101039409637451,
            0.8113007545471191,
            0.7297658920288086,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[2]) == pytest.approx(
        [
            1.0889452695846558,
            0.19699685275554657,
            -0.6078668832778931,
            0.5587908625602722,
            0.8887354731559753,
            0.7994185090065002,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[3]) == pytest.approx(
        [
            0.9778566956520081,
            0.4677884578704834,
            0.5526347160339355,
            -0.32882529497146606,
            -0.8555141687393188,
            0.27526429295539856,
        ],
        abs=1e-7,
        rel=0.0,
    )


def test_wasm_linked_tinygrad_conv2d_matches_upstream_sample(tmp_path: Path) -> None:
    require_wasm_toolchain()
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "tinygrad_conv2d_wasm.py"
    src.write_text(
        "from tinygrad import Tensor, nn\n"
        "Tensor.manual_seed(42)\n"
        "conv = nn.Conv2d(1, 1, 3)\n"
        "x = Tensor.arange(16).reshape(1, 1, 4, 4).float()\n"
        "print(conv.weight.to_list())\n"
        "print(conv.bias.to_list())\n"
        "print(conv(x).to_list())\n",
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)

    assert run.returncode == 0, run.stderr
    lines = [ast.literal_eval(line) for line in run.stdout.strip().splitlines()]
    assert _flatten_numeric(lines[0]) == pytest.approx(
        [
            -0.21733888983726501,
            -0.22886650264263153,
            0.20126104354858398,
            0.2851662039756775,
            -0.2365218847990036,
            0.19731943309307098,
            0.005402088165283203,
            -0.004575650207698345,
            -0.13713280856609344,
        ],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[1]) == pytest.approx(
        [-0.27590489387512207],
        abs=1e-7,
        rel=0.0,
    )
    assert _flatten_numeric(lines[2]) == pytest.approx(
        [
            -0.32956963777542114,
            -0.4648566246032715,
            -0.8707174062728882,
            -1.0060044527053833,
        ],
        abs=1e-7,
        rel=0.0,
    )
