from __future__ import annotations

import math
import os
import sys
from pathlib import Path

import pytest

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process


def _native_molt_env(root: Path) -> dict[str, str]:
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="tinygrad-sdpa",
        session_id=os.environ.get("MOLT_SESSION_ID") or "tinygrad-sdpa",
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_STDLIB_PROFILE"] = "full"
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def _dot(left: list[float], right: list[float]) -> float:
    return sum(l_val * r_val for l_val, r_val in zip(left, right, strict=True))


def _softmax(values: list[float]) -> list[float]:
    max_value = max(values)
    exps = [math.exp(value - max_value) for value in values]
    total = sum(exps)
    return [value / total for value in exps]


def _manual_sdpa(
    query: list[list[list[list[float]]]],
    key: list[list[list[list[float]]]],
    value: list[list[list[list[float]]]],
    mask: list[list[list[list[float]]]] | None,
    scale: float,
) -> list[list[list[list[float]]]]:
    batches = len(query)
    heads = len(query[0])
    query_len = len(query[0][0])
    key_len = len(key[0][0])
    value_dim = len(value[0][0][0])

    out = []
    for batch in range(batches):
        batch_out = []
        for head in range(heads):
            head_out = []
            for q_idx in range(query_len):
                logits = []
                for k_idx in range(key_len):
                    logit = _dot(query[batch][head][q_idx], key[batch][head][k_idx])
                    logit *= scale
                    if mask is not None:
                        logit += mask[batch][head][q_idx][k_idx]
                    logits.append(logit)
                weights = _softmax(logits)
                row = []
                for dim in range(value_dim):
                    row.append(
                        sum(
                            weights[k_idx] * value[batch][head][k_idx][dim]
                            for k_idx in range(key_len)
                        )
                    )
                head_out.append(row)
            batch_out.append(head_out)
        out.append(batch_out)
    return out


def _assert_nested_close(actual: object, expected: object) -> None:
    if isinstance(expected, list):
        assert isinstance(actual, list)
        assert len(actual) == len(expected)
        for actual_item, expected_item in zip(actual, expected, strict=True):
            _assert_nested_close(actual_item, expected_item)
        return
    assert float(actual) == pytest.approx(float(expected), rel=1.0e-6, abs=1.0e-6)


def test_public_tinygrad_sdpa_routes_to_molt_tensor() -> None:
    from molt.gpu.tensor import Tensor as MoltTensor
    from tinygrad import Tensor
    from tinygrad.tensor import Tensor as TensorFromModule

    assert Tensor is TensorFromModule
    assert Tensor is MoltTensor
    assert hasattr(Tensor, "scaled_dot_product_attention")

    query_values = [[[[1.0, 0.5], [0.25, 1.25]]]]
    key_values = [[[[0.75, -0.25], [0.5, 1.0]]]]
    value_values = [[[[2.0, -1.0], [0.5, 3.0]]]]
    additive_mask_values = [[[[0.0, -0.75], [-0.25, 0.0]]]]
    shape = (1, 1, 2, 2)
    scale = 0.5

    query = Tensor([1.0, 0.5, 0.25, 1.25], shape=shape)
    key = TensorFromModule([0.75, -0.25, 0.5, 1.0], shape=shape)
    value = TensorFromModule([2.0, -1.0, 0.5, 3.0], shape=shape)
    additive_mask = Tensor([0.0, -0.75, -0.25, 0.0], shape=shape)

    unmasked = query.scaled_dot_product_attention(
        key,
        value,
        scale=scale,
        is_causal=False,
    )
    masked = query.scaled_dot_product_attention(
        key,
        value,
        attn_mask=additive_mask,
        scale=scale,
        is_causal=False,
    )

    assert unmasked.shape == shape
    assert masked.shape == shape
    _assert_nested_close(
        unmasked.to_list(),
        _manual_sdpa(query_values, key_values, value_values, None, scale),
    )
    _assert_nested_close(
        masked.to_list(),
        _manual_sdpa(
            query_values,
            key_values,
            value_values,
            additive_mask_values,
            scale,
        ),
    )


def test_public_tinygrad_sdpa_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "tinygrad_public_sdpa_probe.py"
    probe.write_text(
        "from tinygrad import Tensor\n"
        "q = Tensor([1.0, 0.0, 0.0, 1.0], shape=(1, 1, 2, 2))\n"
        "k = Tensor([1.0, 0.0, 0.0, 1.0], shape=(1, 1, 2, 2))\n"
        "v = Tensor([10.0, 1.0, 2.0, 20.0], shape=(1, 1, 2, 2))\n"
        "mask = Tensor([0.0, -1.0e9, -1.0e9, 0.0], shape=(1, 1, 2, 2))\n"
        "out = q.scaled_dot_product_attention(k, v, attn_mask=mask, scale=1.0, is_causal=False)\n"
        "print(out.to_list())\n",
        encoding="utf-8",
    )

    run = run_native_test_process(
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
        timeout=240,
        check=False,
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip() == "[[[[10.0, 1.0], [2.0, 20.0]]]]"
