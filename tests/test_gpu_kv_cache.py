from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest


def _native_molt_env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env["MOLT_EXT_ROOT"] = str(root)
    env["MOLT_SESSION_ID"] = "test-gpu-kv-cache"
    env["CARGO_TARGET_DIR"] = str(root / "target" / "test-gpu-kv-cache")
    env["MOLT_DIFF_CARGO_TARGET_DIR"] = env["CARGO_TARGET_DIR"]
    env["MOLT_CACHE"] = str(root / ".molt_cache")
    env["MOLT_DIFF_ROOT"] = str(root / "tmp" / "diff")
    env["MOLT_DIFF_TMPDIR"] = str(root / "tmp")
    env["UV_CACHE_DIR"] = str(root / ".uv-cache")
    env["TMPDIR"] = str(root / "tmp")
    env["MOLT_HERMETIC_MODULE_ROOTS"] = "1"
    env["MOLT_BACKEND_DAEMON"] = "0"
    return env


def test_dense_kv_cache_attention_matches_tensor_sdpa():
    from molt.gpu.kv_cache import DenseKVCache
    from molt.gpu.tensor import Tensor, tensor_scaled_dot_product_attention

    q = Tensor([0.5, -0.1, 0.4, 0.2], shape=(1, 1, 1, 4))
    k0 = Tensor([0.6, -0.2, 0.1, 0.4], shape=(1, 1, 1, 4))
    v0 = Tensor([0.2, 0.1, -0.3, 0.4], shape=(1, 1, 1, 4))
    k1 = Tensor([0.1, 0.5, -0.3, -0.2], shape=(1, 1, 1, 4))
    v1 = Tensor([-0.5, 0.2, 0.4, -0.1], shape=(1, 1, 1, 4))

    cache = DenseKVCache()
    cache.append(k0, v0)
    cache.append(k1, v1)

    out = cache.attention(q, scale=1.0)

    manual = tensor_scaled_dot_product_attention(
        q,
        Tensor([0.6, -0.2, 0.1, 0.4, 0.1, 0.5, -0.3, -0.2], shape=(1, 1, 2, 4)),
        Tensor([0.2, 0.1, -0.3, 0.4, -0.5, 0.2, 0.4, -0.1], shape=(1, 1, 2, 4)),
        None,
        1.0,
    )

    assert len(cache) == 2
    assert out.shape == (1, 1, 1, 4)
    assert out.reshape(4).to_list() == pytest.approx(manual.reshape(4).to_list())


def test_dense_kv_cache_supports_grouped_query_attention():
    from molt.gpu.kv_cache import DenseKVCache
    from molt.gpu.tensor import Tensor, tensor_scaled_dot_product_attention

    q = Tensor(
        [
            0.5, -0.1,
            0.2, 0.3,
            -0.4, 0.6,
            0.1, -0.2,
        ],
        shape=(1, 4, 1, 2),
    )
    k = Tensor(
        [
            0.6, -0.2,
            0.1, 0.4,
            -0.3, 0.5,
            0.2, -0.1,
        ],
        shape=(1, 2, 2, 2),
    )
    v = Tensor(
        [
            0.2, 0.1,
            -0.3, 0.4,
            -0.5, 0.2,
            0.4, -0.1,
        ],
        shape=(1, 2, 2, 2),
    )

    cache = DenseKVCache()
    cache.append(k, v)

    out = cache.attention(q, scale=1.0)

    manual = tensor_scaled_dot_product_attention(
        q,
        k.repeat_axis(1, 2),
        v.repeat_axis(1, 2),
        None,
        1.0,
    )

    assert out.shape == (1, 4, 1, 2)
    assert out.reshape(8).to_list() == pytest.approx(manual.reshape(8).to_list())


def test_tensor_sdpa_routes_cache_views_through_cache_backend():
    from molt.gpu.kv_cache import DenseKVCache
    from molt.gpu.tensor import Tensor, tensor_scaled_dot_product_attention

    q = Tensor([0.5, -0.1, 0.4, 0.2], shape=(1, 1, 1, 4))
    k = Tensor([0.6, -0.2, 0.1, 0.4], shape=(1, 1, 1, 4))
    v = Tensor([0.2, 0.1, -0.3, 0.4], shape=(1, 1, 1, 4))

    cache = DenseKVCache()
    cache.append(k, v)

    out = tensor_scaled_dot_product_attention(
        q,
        cache.keys(),
        cache.values(),
        None,
        1.0,
    )

    manual = cache.attention(q, scale=1.0)

    assert out.reshape(4).to_list() == pytest.approx(manual.reshape(4).to_list())


def test_tensor_sdpa_cache_views_bypass_fused_intrinsic(monkeypatch):
    import molt.gpu.tensor as tensor_mod
    from molt.gpu.kv_cache import DenseKVCache
    from molt.gpu.tensor import Tensor, tensor_scaled_dot_product_attention

    monkeypatch.setenv("MOLT_GPU_BACKEND", "webgpu")
    monkeypatch.setattr(
        tensor_mod,
        "_MOLT_GPU_TENSOR_SCALED_DOT_PRODUCT_ATTENTION",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("fused intrinsic should not run on cache views")
        ),
    )

    q = Tensor([0.5, -0.1, 0.4, 0.2], shape=(1, 1, 1, 4))
    k = Tensor([0.6, -0.2, 0.1, 0.4], shape=(1, 1, 1, 4))
    v = Tensor([0.2, 0.1, -0.3, 0.4], shape=(1, 1, 1, 4))
    cache = DenseKVCache()
    cache.append(k, v)

    out = tensor_scaled_dot_product_attention(q, cache.keys(), cache.values(), None, 1.0)

    assert out.shape == (1, 1, 1, 4)


def test_turboquant_attention_kv_cache_matches_rowwise_helper():
    from molt.gpu.kv_cache import TurboQuantAttentionKVCache
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec, TurboQuantKVCache

    codec = TurboQuantCodec(dim=8, bits=3, seed=5, qjl_seed=19)
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
    query = Tensor([0.5, -0.1, 0.4, 0.2, -0.3, 0.6, -0.2, 0.1], shape=(1, 1, 1, 8))

    helper = TurboQuantKVCache.from_tensors(codec, keys, values)
    cache = TurboQuantAttentionKVCache(codec)
    cache.append(keys.reshape(1, 1, 3, 8), values.reshape(1, 1, 3, 8))

    out = cache.attention(query, scale=1.0)

    assert len(cache) == 3
    assert out.shape == (1, 1, 1, 8)
    assert out.reshape(8).to_list() == pytest.approx(
        helper.attention_output(query.reshape(8)).to_list()
    )


def test_turboquant_attention_kv_cache_uses_intrinsic_when_available(monkeypatch):
    import molt.gpu.kv_cache as kv_cache_mod
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec

    codec = TurboQuantCodec(dim=2, bits=3, seed=5, qjl_seed=19)
    cache = kv_cache_mod.TurboQuantAttentionKVCache(codec)
    cache.append(
        Tensor([0.6, -0.2, 0.1, 0.4], shape=(1, 1, 2, 2)),
        Tensor([0.2, 0.1, -0.3, 0.4], shape=(1, 1, 2, 2)),
    )
    q = Tensor([0.5, -0.1], shape=(1, 1, 1, 2))
    seen = {}

    def fake_intrinsic(q_arg, k_arg, v_arg, mask_arg, scale_arg):
        seen["q_shape"] = q_arg.shape
        seen["k_role"] = k_arg._kv_role
        seen["v_role"] = v_arg._kv_role
        seen["same_cache"] = k_arg._kv_cache is v_arg._kv_cache
        seen["mask"] = mask_arg
        seen["scale"] = scale_arg
        return Tensor([1.0, 2.0], shape=(1, 1, 1, 2))

    monkeypatch.setattr(kv_cache_mod, "_MOLT_GPU_TURBOQUANT_ATTENTION_PACKED", fake_intrinsic)

    out = cache.attention(q, scale=1.0)

    assert out.to_list() == [[[[1.0, 2.0]]]]
    assert seen == {
        "q_shape": (1, 1, 1, 2),
        "k_role": "key",
        "v_role": "value",
        "same_cache": True,
        "mask": None,
        "scale": 1.0,
    }


def test_turboquant_attention_kv_cache_reuses_decoded_values_across_attention_calls(monkeypatch):
    from molt.gpu.kv_cache import TurboQuantAttentionKVCache
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec

    codec = TurboQuantCodec(dim=8, bits=3, seed=5, qjl_seed=19)
    cache = TurboQuantAttentionKVCache(codec)
    cache.append(
        Tensor(
            [
                0.6, -0.2, 0.1, 0.4, -0.5, 0.3, 0.2, 0.1,
                0.1, 0.5, -0.3, -0.2, 0.6, -0.1, 0.4, -0.4,
            ],
            shape=(1, 1, 2, 8),
        ),
        Tensor(
            [
                0.2, 0.1, -0.3, 0.4, 0.5, -0.2, 0.6, -0.1,
                -0.5, 0.2, 0.4, -0.1, 0.3, 0.7, -0.2, 0.6,
            ],
            shape=(1, 1, 2, 8),
        ),
    )
    query = Tensor([0.5, -0.1, 0.4, 0.2, -0.3, 0.6, -0.2, 0.1], shape=(1, 1, 1, 8))
    calls = {"count": 0}
    original_dequantize = codec.dequantize

    def tracked_dequantize(encoded):
        calls["count"] += 1
        return original_dequantize(encoded)

    monkeypatch.setattr(codec, "dequantize", tracked_dequantize)

    first = cache._attention_reference(query, None, 1.0)
    second = cache._attention_reference(query, None, 1.0)

    assert first.reshape(8).to_list() == pytest.approx(second.reshape(8).to_list())
    assert calls["count"] == 2


def test_turboquant_attention_kv_cache_supports_grouped_query_attention():
    from molt.gpu.kv_cache import TurboQuantAttentionKVCache
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec, TurboQuantKVCache

    codec = TurboQuantCodec(dim=2, bits=3, seed=5, qjl_seed=19)
    k = Tensor(
        [
            0.6, -0.2,
            0.1, 0.4,
            -0.3, 0.5,
            0.2, -0.1,
        ],
        shape=(1, 2, 2, 2),
    )
    v = Tensor(
        [
            0.2, 0.1,
            -0.3, 0.4,
            -0.5, 0.2,
            0.4, -0.1,
        ],
        shape=(1, 2, 2, 2),
    )
    q = Tensor(
        [
            0.5, -0.1,
            0.2, 0.3,
            -0.4, 0.6,
            0.1, -0.2,
        ],
        shape=(1, 4, 1, 2),
    )

    cache = TurboQuantAttentionKVCache(codec)
    cache.append(k, v)
    out = cache.attention(q, scale=1.0)

    q_rows = q.to_list()[0]
    k_rows = k.to_list()[0]
    v_rows = v.to_list()[0]
    manual = []
    for head_index, q_head in enumerate(q_rows):
        shared_head = head_index // 2
        helper = TurboQuantKVCache.from_tensors(
            codec,
            Tensor(k_rows[shared_head]),
            Tensor(v_rows[shared_head]),
        )
        manual.append([helper.attention_output(Tensor(q_head[0])).to_list()])

    assert out.shape == (1, 4, 1, 2)
    assert out.reshape(8).to_list() == pytest.approx(Tensor(manual).reshape(8).to_list())


def test_turboquant_attention_kv_cache_broadcasts_singleton_mask_width():
    from molt.gpu.kv_cache import TurboQuantAttentionKVCache
    from molt.gpu.tensor import Tensor
    from molt.gpu.turboquant import TurboQuantCodec

    codec = TurboQuantCodec(dim=8, bits=3, seed=5, qjl_seed=19)
    cache = TurboQuantAttentionKVCache(codec)
    cache.append(
        Tensor(
            [
                0.6, -0.2, 0.1, 0.4, -0.5, 0.3, 0.2, 0.1,
                0.1, 0.5, -0.3, -0.2, 0.6, -0.1, 0.4, -0.4,
            ],
            shape=(1, 1, 2, 8),
        ),
        Tensor(
            [
                0.2, 0.1, -0.3, 0.4, 0.5, -0.2, 0.6, -0.1,
                -0.5, 0.2, 0.4, -0.1, 0.3, 0.7, -0.2, 0.6,
            ],
            shape=(1, 1, 2, 8),
        ),
    )
    query = Tensor([0.5, -0.1, 0.4, 0.2, -0.3, 0.6, -0.2, 0.1], shape=(1, 1, 1, 8))

    unmasked = cache.attention(query, scale=1.0)
    masked = cache.attention(
        query,
        scale=1.0,
        mask=Tensor([0.0], shape=(1, 1, 1, 1)),
    )

    assert masked.reshape(8).to_list() == pytest.approx(unmasked.reshape(8).to_list())


def test_dense_kv_cache_defers_concat_until_materialization(monkeypatch):
    from molt.gpu.kv_cache import DenseKVCache
    from molt.gpu.tensor import Tensor

    cache = DenseKVCache()
    left = Tensor([0.6, -0.2, 0.1, 0.4], shape=(1, 1, 1, 4))
    right = Tensor([0.1, 0.5, -0.3, -0.2], shape=(1, 1, 1, 4))
    value_left = Tensor([0.2, 0.1, -0.3, 0.4], shape=(1, 1, 1, 4))
    value_right = Tensor([-0.5, 0.2, 0.4, -0.1], shape=(1, 1, 1, 4))
    q = Tensor([0.5, -0.1, 0.4, 0.2], shape=(1, 1, 1, 4))

    calls = []
    original_cat = Tensor.cat

    def tracked_cat(self, other, dim=0):
        calls.append((self.shape, other.shape, dim))
        return original_cat(self, other, dim=dim)

    monkeypatch.setattr(Tensor, "cat", tracked_cat)

    cache.append(left, value_left)
    cache.append(right, value_right)

    assert calls == []

    cache.attention(q, scale=1.0)
    assert len(calls) == 2

    cache.attention(q, scale=1.0)
    assert len(calls) == 2


def test_dense_kv_cache_reuses_grouped_query_expansions(monkeypatch):
    import molt.gpu.kv_cache as kv_cache_mod
    from molt.gpu.kv_cache import DenseKVCache
    from molt.gpu.tensor import Tensor

    cache = DenseKVCache()
    cache.append(
        Tensor(
            [
                0.6, -0.2,
                0.1, 0.4,
                -0.3, 0.5,
                0.2, -0.1,
            ],
            shape=(1, 2, 2, 2),
        ),
        Tensor(
            [
                0.2, 0.1,
                -0.3, 0.4,
                -0.5, 0.2,
                0.4, -0.1,
            ],
            shape=(1, 2, 2, 2),
        ),
    )
    q = Tensor(
        [
            0.5, -0.1,
            0.2, 0.3,
            -0.4, 0.6,
            0.1, -0.2,
        ],
        shape=(1, 4, 1, 2),
    )
    repeat_calls = []
    permute_calls = []
    original_repeat = Tensor.repeat_axis
    original_permute = kv_cache_mod.tensor_permute_dims

    def tracked_repeat(self, axis, repeats):
        repeat_calls.append((self.shape, axis, repeats))
        return original_repeat(self, axis, repeats)

    def tracked_permute(tensor, dims):
        permute_calls.append((tensor.shape, tuple(dims)))
        return original_permute(tensor, dims)

    monkeypatch.setattr(Tensor, "repeat_axis", tracked_repeat)
    monkeypatch.setattr(kv_cache_mod, "tensor_permute_dims", tracked_permute)

    first = cache.attention(q, scale=1.0)
    second = cache.attention(q, scale=1.0)

    assert first.reshape(8).to_list() == pytest.approx(second.reshape(8).to_list())
    assert repeat_calls == [
        ((1, 2, 2, 2), 1, 2),
        ((1, 2, 2, 2), 1, 2),
    ]
    assert permute_calls == [
        ((1, 4, 2, 2), (0, 1, 3, 2)),
    ]


def test_transformer_multihead_attention_routes_through_cache(monkeypatch):
    import molt.gpu.transformer as transformer_mod
    from molt.gpu.tensor import Tensor

    attn = transformer_mod.MultiHeadAttention(embed_dim=4, num_heads=2, causal=True)
    attn.q_proj = lambda x: x
    attn.k_proj = lambda x: x
    attn.v_proj = lambda x: x
    attn.out_proj = lambda x: x

    seen = {}

    class FakeCache:
        def __init__(self):
            self.count = 0

        def __len__(self):
            return self.count

        def append(self, k, v):
            seen["k_shape"] = k.shape
            seen["v_shape"] = v.shape
            self.count += k.shape[2]

        def attention(self, q, *, scale, mask=None):
            raise AssertionError("transformer cache path should go through tensor sdpa")

        def keys(self):
            return "keys-view"

        def values(self):
            return "values-view"

    def fake_sdpa(q, k, v, mask, scale):
            seen["q_shape"] = q.shape
            seen["k_view"] = k
            seen["v_view"] = v
            seen["scale"] = scale
            seen["mask"] = mask
            return q

    monkeypatch.setattr(transformer_mod, "tensor_scaled_dot_product_attention", fake_sdpa)

    x = Tensor([[1.0, 2.0, 3.0, 4.0]])
    out = attn(x, kv_cache=FakeCache())

    assert out.shape == (1, 4)
    assert seen["q_shape"] == (1, 2, 1, 2)
    assert seen["k_shape"] == (1, 2, 1, 2)
    assert seen["v_shape"] == (1, 2, 1, 2)
    assert seen["k_view"] == "keys-view"
    assert seen["v_view"] == "values-view"
    assert seen["scale"] == attn.scale
    assert seen["mask"].shape == (1, 1, 1, 1)
    assert seen["mask"].to_list() == [[[[0.0]]]]


def test_transformer_multihead_attention_rolls_back_cache_on_failure():
    import molt.gpu.transformer as transformer_mod
    from molt.gpu.tensor import Tensor

    attn = transformer_mod.MultiHeadAttention(embed_dim=4, num_heads=2, causal=True)
    attn.q_proj = lambda x: x
    attn.k_proj = lambda x: x
    attn.v_proj = lambda x: x
    attn.out_proj = lambda x: x

    class FakeCache:
        def __init__(self):
            self.count = 3

        def __len__(self):
            return self.count

        def append(self, k, v):
            self.count += k.shape[2]

        def truncate(self, length):
            self.count = length

        def attention(self, q, *, scale, mask=None):
            raise RuntimeError("boom")

        def keys(self):
            class KeyView:
                _kv_role = "key"

                def __init__(self, cache):
                    self._kv_cache = cache

            return KeyView(self)

        def values(self):
            class ValueView:
                _kv_role = "value"

                def __init__(self, cache):
                    self._kv_cache = cache

            return ValueView(self)

    cache = FakeCache()
    x = Tensor([[1.0, 2.0, 3.0, 4.0]])

    with pytest.raises(RuntimeError, match="boom"):
        attn(x, kv_cache=cache)

    assert len(cache) == 3


def test_transformer_decoder_offsets_position_ids_by_cache_prefix():
    import molt.gpu.transformer as transformer_mod
    from molt.gpu.tensor import Tensor

    decoder = transformer_mod.TransformerDecoder(
        vocab_size=8,
        embed_dim=4,
        num_heads=2,
        num_layers=1,
        max_seq_len=16,
    )

    seen = {}

    decoder.token_embedding = lambda token_ids: Tensor([[1.0, 1.0, 1.0, 1.0]])

    def fake_position_embedding(indices):
        seen["pos_ids"] = list(indices)
        return Tensor([[0.0, 0.0, 0.0, 0.0]])

    class IdentityBlock:
        def __call__(self, x, kv_cache=None):
            seen["block_cache"] = kv_cache
            return x

    decoder.position_embedding = fake_position_embedding
    decoder.blocks = [IdentityBlock()]
    decoder.ln_final = lambda x: x
    decoder.lm_head = lambda x: x

    class FakeCache:
        def __len__(self):
            return 3

    out = decoder([7], kv_caches=[FakeCache()])

    assert out.shape == (1, 4)
    assert seen["pos_ids"] == [3]
    assert seen["block_cache"] is not None


def test_transformer_decoder_rejects_inconsistent_cache_prefix_lengths():
    import molt.gpu.transformer as transformer_mod

    decoder = transformer_mod.TransformerDecoder(
        vocab_size=8,
        embed_dim=4,
        num_heads=2,
        num_layers=2,
        max_seq_len=16,
    )

    class Cache3:
        def __len__(self):
            return 3

    class Cache4:
        def __len__(self):
            return 4

    with pytest.raises(ValueError, match="same prefix length"):
        decoder([7], kv_caches=[Cache3(), Cache4()])


def test_dense_kv_cache_compiles_in_native_molt(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    probe = tmp_path / "gpu_kv_cache_native.py"
    probe.write_text(
        "from molt.gpu.kv_cache import DenseKVCache\n"
        "from molt.gpu.tensor import Tensor\n"
        "\n"
        "cache = DenseKVCache()\n"
        "cache.append(\n"
        "    Tensor([\n"
        "        0.6, -0.2,\n"
        "        0.1, 0.4,\n"
        "        -0.3, 0.5,\n"
        "        0.2, -0.1,\n"
        "    ], shape=(1, 2, 2, 2)),\n"
        "    Tensor([\n"
        "        0.2, 0.1,\n"
        "        -0.3, 0.4,\n"
        "        -0.5, 0.2,\n"
        "        0.4, -0.1,\n"
        "    ], shape=(1, 2, 2, 2)),\n"
        ")\n"
        "out = cache.attention(\n"
        "    Tensor([\n"
        "        0.5, -0.1,\n"
        "        0.2, 0.3,\n"
        "        -0.4, 0.6,\n"
        "        0.1, -0.2,\n"
        "    ], shape=(1, 4, 1, 2)),\n"
        "    scale=1.0,\n"
        ")\n"
        "print(len(cache))\n"
        "print(out.shape)\n",
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
    assert run.stdout.strip().splitlines() == [
        "2",
        "(1, 4, 1, 2)",
    ]
