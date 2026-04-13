from __future__ import annotations

import math
import os
import subprocess
import sys
from pathlib import Path

import pytest


def _native_molt_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_EXT_ROOT"] = str(root)
    env["CARGO_TARGET_DIR"] = str(root / "target")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(root / ".molt_cache")
    env["MOLT_DIFF_ROOT"] = str(root / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(root / "tmp")
    env["UV_CACHE_DIR"] = str(root / ".uv-cache")
    env["TMPDIR"] = str(root / "tmp")
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def test_turboquant_codec_rejects_non_power_of_two_hadamard_dimensions():
    from molt.gpu.turboquant import TurboQuantCodec

    with pytest.raises(ValueError, match="power-of-two"):
        TurboQuantCodec(dim=6, bits=3, seed=7, rotation="hadamard")


def test_turboquant_prod_mean_estimate_is_closer_to_exact_than_mse_only():
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec

    vector = Tensor([0.7, -0.3, 0.2, 0.5, -0.1, 0.4, -0.2, 0.1])
    query = Tensor([0.6, 0.2, -0.5, 0.1, 0.3, -0.4, 0.2, 0.7])
    exact = float((vector * query).sum().item())

    mse_codec = TurboQuantCodec(dim=8, bits=3, seed=11, qjl_seed=101)
    mse_encoded = mse_codec.quantize_mse(vector)
    mse_estimate = mse_codec.estimate_mse_inner_product(query, mse_encoded)

    estimates = []
    for qjl_seed in range(32):
        codec = TurboQuantCodec(dim=8, bits=3, seed=11, qjl_seed=qjl_seed)
        encoded = codec.quantize_prod(vector)
        estimates.append(codec.estimate_inner_product(query, encoded))

    mean_prod_estimate = sum(estimates) / len(estimates)

    assert abs(mean_prod_estimate - exact) < abs(mse_estimate - exact)


def test_turboquant_prepared_query_matches_direct_estimates():
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec

    codec = TurboQuantCodec(dim=8, bits=3, seed=11, qjl_seed=19)
    query = Tensor([0.6, 0.2, -0.5, 0.1, 0.3, -0.4, 0.2, 0.7])
    vector = Tensor([0.7, -0.3, 0.2, 0.5, -0.1, 0.4, -0.2, 0.1])
    mse_encoded = codec.quantize_mse(vector)
    prod_encoded = codec.quantize_prod(vector)

    prepared = codec.prepare_query(query)

    assert codec.estimate_mse_inner_product_prepared(prepared, mse_encoded) == pytest.approx(
        codec.estimate_mse_inner_product(query, mse_encoded)
    )
    assert codec.estimate_inner_product_prepared(prepared, prod_encoded) == pytest.approx(
        codec.estimate_inner_product(query, prod_encoded)
    )


def test_turboquant_prepared_estimates_do_not_depend_on_codec_codebook_after_encode():
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec

    codec = TurboQuantCodec(dim=8, bits=3, seed=11, qjl_seed=19)
    query = Tensor([0.6, 0.2, -0.5, 0.1, 0.3, -0.4, 0.2, 0.7])
    vector = Tensor([0.7, -0.3, 0.2, 0.5, -0.1, 0.4, -0.2, 0.1])
    mse_encoded = codec.quantize_mse(vector)
    prod_encoded = codec.quantize_prod(vector)
    prepared = codec.prepare_query(query)
    mse_expected = codec.estimate_mse_inner_product_prepared(prepared, mse_encoded)
    prod_expected = codec.estimate_inner_product_prepared(prepared, prod_encoded)

    codec.codebook = None

    assert codec.estimate_mse_inner_product_prepared(prepared, mse_encoded) == pytest.approx(
        mse_expected
    )
    assert codec.estimate_inner_product_prepared(prepared, prod_encoded) == pytest.approx(
        prod_expected
    )


def test_turboquant_kv_cache_attention_output_matches_manual_reference():
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec, TurboQuantKVCache

    keys = Tensor(
        [
            [0.6, -0.2, 0.1, 0.4, -0.5, 0.3, 0.2, 0.1],
            [0.1, 0.5, -0.3, -0.2, 0.6, -0.1, 0.4, -0.4],
            [-0.2, 0.3, 0.5, -0.6, 0.1, 0.2, -0.4, 0.7],
        ]
    )
    values = Tensor(
        [
            [0.2, 0.1, -0.3, 0.4, 0.5, -0.2, 0.6, -0.1],
            [-0.5, 0.2, 0.4, -0.1, 0.3, 0.7, -0.2, 0.6],
            [0.3, -0.4, 0.2, 0.1, -0.6, 0.5, 0.4, -0.3],
        ]
    )
    query = Tensor([0.5, -0.1, 0.4, 0.2, -0.3, 0.6, -0.2, 0.1])

    codec = TurboQuantCodec(dim=8, bits=3, seed=5, qjl_seed=19)
    cache = TurboQuantKVCache.from_tensors(codec, keys, values)

    logits = cache.attention_logits(query)
    output = cache.attention_output(query)

    assert logits.shape == (3,)
    assert output.shape == (8,)

    weights = logits.softmax().to_list()
    decoded_values = [codec.dequantize(encoded).to_list() for encoded in cache.value_vectors]
    manual = []
    for dim_index in range(8):
        acc = 0.0
        for row_index, row in enumerate(decoded_values):
            acc += weights[row_index] * row[dim_index]
        manual.append(acc)

    assert output.to_list() == pytest.approx(manual)


def test_turboquant_kv_cache_prepares_query_once_per_logits_call(monkeypatch):
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec, TurboQuantKVCache

    codec = TurboQuantCodec(dim=8, bits=3, seed=5, qjl_seed=19)
    cache = TurboQuantKVCache.from_tensors(
        codec,
        Tensor(
            [
                [0.6, -0.2, 0.1, 0.4, -0.5, 0.3, 0.2, 0.1],
                [0.1, 0.5, -0.3, -0.2, 0.6, -0.1, 0.4, -0.4],
            ]
        ),
        Tensor(
            [
                [0.2, 0.1, -0.3, 0.4, 0.5, -0.2, 0.6, -0.1],
                [-0.5, 0.2, 0.4, -0.1, 0.3, 0.7, -0.2, 0.6],
            ]
        ),
    )
    query = Tensor([0.5, -0.1, 0.4, 0.2, -0.3, 0.6, -0.2, 0.1])
    calls = {"count": 0}
    original_prepare = codec.prepare_query

    def tracked_prepare(query_arg):
        calls["count"] += 1
        return original_prepare(query_arg)

    monkeypatch.setattr(codec, "prepare_query", tracked_prepare)

    logits = cache.attention_logits(query)

    assert logits.shape == (2,)
    assert calls["count"] == 1


def test_turboquant_kv_cache_reuses_decoded_values_across_attention_calls(monkeypatch):
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec, TurboQuantKVCache

    codec = TurboQuantCodec(dim=8, bits=3, seed=5, qjl_seed=19)
    cache = TurboQuantKVCache.from_tensors(
        codec,
        Tensor(
            [
                [0.6, -0.2, 0.1, 0.4, -0.5, 0.3, 0.2, 0.1],
                [0.1, 0.5, -0.3, -0.2, 0.6, -0.1, 0.4, -0.4],
            ]
        ),
        Tensor(
            [
                [0.2, 0.1, -0.3, 0.4, 0.5, -0.2, 0.6, -0.1],
                [-0.5, 0.2, 0.4, -0.1, 0.3, 0.7, -0.2, 0.6],
            ]
        ),
    )
    query = Tensor([0.5, -0.1, 0.4, 0.2, -0.3, 0.6, -0.2, 0.1])
    calls = {"count": 0}
    original_dequantize = codec.dequantize

    def tracked_dequantize(encoded):
        calls["count"] += 1
        return original_dequantize(encoded)

    monkeypatch.setattr(codec, "dequantize", tracked_dequantize)

    first = cache.attention_output(query)
    second = cache.attention_output(query)

    assert first.to_list() == pytest.approx(second.to_list())
    assert calls["count"] == len(cache.value_vectors)


def test_turboquant_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "gpu_turboquant_native.py"
    probe.write_text(
        "from molt.gpu.tensor import Tensor\n"
        "from molt.gpu.turboquant import TurboQuantCodec, TurboQuantKVCache\n"
        "\n"
        "codec = TurboQuantCodec(dim=8, bits=3, seed=5, qjl_seed=19)\n"
        "vector = Tensor([0.7, -0.3, 0.2, 0.5, -0.1, 0.4, -0.2, 0.1])\n"
        "query = Tensor([0.6, 0.2, -0.5, 0.1, 0.3, -0.4, 0.2, 0.7])\n"
        "encoded = codec.quantize_prod(vector)\n"
        "print(round(codec.estimate_inner_product(query, encoded), 6))\n"
        "cache = TurboQuantKVCache.from_tensors(\n"
        "    codec,\n"
        "    Tensor([\n"
        "        [0.6, -0.2, 0.1, 0.4, -0.5, 0.3, 0.2, 0.1],\n"
        "        [0.1, 0.5, -0.3, -0.2, 0.6, -0.1, 0.4, -0.4],\n"
        "    ]),\n"
        "    Tensor([\n"
        "        [0.2, 0.1, -0.3, 0.4, 0.5, -0.2, 0.6, -0.1],\n"
        "        [-0.5, 0.2, 0.4, -0.1, 0.3, 0.7, -0.2, 0.6],\n"
        "    ]),\n"
        ")\n"
        "print(cache.attention_logits(query).shape)\n"
        "print(cache.attention_output(query).shape)\n",
        encoding="utf-8",
    )

    run = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(probe),
        ],
        cwd=root,
        env=_native_molt_env(root),
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    lines = run.stdout.strip().splitlines()
    assert len(lines) == 3
    assert lines[1] == "(2,)"
    assert lines[2] == "(8,)"
