from __future__ import annotations

import importlib
import struct
import sys
import types

import pytest


MODULE_NAMES = (
    "molt.gpu.tensor",
    "molt.gpu.interop",
    "molt.gpu.kv_cache",
)
RELATED_MODULE_NAMES = MODULE_NAMES + (
    "molt.gpu.turboquant",
)


@pytest.fixture(autouse=True)
def _clear_gpu_modules_after_test():
    yield
    for module_name in RELATED_MODULE_NAMES:
        sys.modules.pop(module_name, None)


def _install_fake_intrinsics(monkeypatch: pytest.MonkeyPatch):
    state = {
        "runtime_active": False,
        "loaded": {},
        "load_calls": [],
        "require_calls": [],
    }
    fake = types.ModuleType("_intrinsics")

    def runtime_active() -> bool:
        return state["runtime_active"]

    def load_intrinsic(name: str, namespace=None):  # type: ignore[no-untyped-def]
        state["load_calls"].append(name)
        return state["loaded"].get(name)

    def require_intrinsic(name: str, namespace=None):  # type: ignore[no-untyped-def]
        state["require_calls"].append(name)
        if not state["runtime_active"]:
            raise RuntimeError("runtime inactive")
        value = state["loaded"].get(name)
        if value is not None:
            return value
        raise RuntimeError(f"intrinsic unavailable: {name}")

    fake.runtime_active = runtime_active  # type: ignore[attr-defined]
    fake.load_intrinsic = load_intrinsic  # type: ignore[attr-defined]
    fake.require_intrinsic = require_intrinsic  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "_intrinsics", fake)
    for module_name in RELATED_MODULE_NAMES:
        sys.modules.pop(module_name, None)
    return state


def test_tensor_optional_intrinsic_retries_and_caches_positive_hit(monkeypatch):
    state = _install_fake_intrinsics(monkeypatch)
    tensor_mod = importlib.import_module("molt.gpu.tensor")
    monkeypatch.setattr(tensor_mod, "_runtime_intrinsics_active", lambda: False)

    sentinel = ["tensor-buffer-to-list"]
    state["runtime_active"] = True
    state["loaded"]["molt_gpu_buffer_to_list"] = lambda _buf, _size: sentinel

    tensor = tensor_mod.Tensor([1.0, 2.0])

    assert tensor_mod.tensor_data_list(tensor) is sentinel
    assert tensor_mod.tensor_data_list(tensor) is sentinel
    assert state["load_calls"].count("molt_gpu_buffer_to_list") == 1
    assert state["require_calls"].count("molt_gpu_buffer_to_list") == 0


def test_interop_optional_intrinsic_retries_and_caches_positive_hit(monkeypatch):
    state = _install_fake_intrinsics(monkeypatch)
    interop_mod = importlib.import_module("molt.gpu.interop")

    sentinel = struct.pack("<2f", 1.5, 2.5)
    state["runtime_active"] = True
    state["loaded"]["molt_gpu_interop_decode_f16_bytes_to_f32"] = lambda raw: sentinel

    meta = {"dtype": "F16", "shape": [2], "data_offsets": [0, 4]}

    first = interop_mod._load_safetensor_entry(b"\x00\x00\x00\x00", 0, meta)
    second = interop_mod._load_safetensor_entry(b"\x00\x00\x00\x00", 0, meta)

    assert first.to_list() == pytest.approx([1.5, 2.5])
    assert second.to_list() == pytest.approx([1.5, 2.5])
    assert state["load_calls"].count("molt_gpu_interop_decode_f16_bytes_to_f32") == 1
    assert state["require_calls"].count("molt_gpu_interop_decode_f16_bytes_to_f32") == 0


def test_kv_cache_optional_intrinsic_retries_and_caches_positive_hit(monkeypatch):
    state = _install_fake_intrinsics(monkeypatch)
    kv_cache_mod = importlib.import_module("molt.gpu.kv_cache")
    tensor_mod = importlib.import_module("molt.gpu.tensor")
    monkeypatch.setattr(tensor_mod, "_runtime_intrinsics_active", lambda: False)

    state["runtime_active"] = True
    state["loaded"]["molt_gpu_turboquant_attention_packed"] = (
        lambda q, k, v, mask, scale: tensor_mod.Tensor([9.0] * 8, shape=(1, 1, 1, 8))
    )

    codec = importlib.import_module("molt.gpu.turboquant").TurboQuantCodec(
        dim=8,
        bits=3,
        seed=5,
        qjl_seed=19,
    )
    cache = kv_cache_mod.TurboQuantAttentionKVCache(codec)
    cache.append(
        tensor_mod.Tensor([0.1] * 8, shape=(1, 1, 1, 8)),
        tensor_mod.Tensor([0.2] * 8, shape=(1, 1, 1, 8)),
    )
    q = tensor_mod.Tensor([0.3] * 8, shape=(1, 1, 1, 8))

    first = cache.attention(q, scale=1.0)
    second = cache.attention(q, scale=1.0)

    assert first.to_list() == [[[[9.0] * 8]]]
    assert second.to_list() == [[[[9.0] * 8]]]
    assert state["load_calls"].count("molt_gpu_turboquant_attention_packed") == 1
    assert state["require_calls"].count("molt_gpu_turboquant_attention_packed") == 0


def test_tensor_optional_intrinsic_raises_when_runtime_active_and_missing(
    monkeypatch,
):
    state = _install_fake_intrinsics(monkeypatch)
    tensor_mod = importlib.import_module("molt.gpu.tensor")

    state["runtime_active"] = True
    tensor = tensor_mod.Tensor([1.0, 2.0])

    with pytest.raises(RuntimeError, match="intrinsic unavailable: molt_gpu_buffer_to_list"):
        tensor_mod.tensor_data_list(tensor)
