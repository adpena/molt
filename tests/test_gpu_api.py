"""GPU API verification tests.

Tests the Python-side GPU infrastructure: Buffer, kernel simulation,
Tensor, DataFrame, ops, and all sub-module imports.
"""

import json
import math
from pathlib import Path
import pytest
import struct
import sys

# ── Core imports ─────────────────────────────────────────────────────────────


def _write_safetensors_fixture(path: Path) -> None:
    tensors = {
        "t0": ([1.0, 2.0], [2]),
        "t1": ([3.0, 4.0, 5.0, 6.0], [2, 2]),
    }
    payload = bytearray()
    header: dict[str, object] = {}
    offset = 0
    for name, (values, shape) in tensors.items():
        raw = struct.pack(f"<{len(values)}f", *values)
        header[name] = {
            "dtype": "F32",
            "shape": shape,
            "data_offsets": [offset, offset + len(raw)],
        }
        payload.extend(raw)
        offset += len(raw)
    header_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    with path.open("wb") as handle:
        handle.write(struct.pack("<Q", len(header_bytes)))
        handle.write(header_bytes)
        handle.write(payload)

def test_core_imports():
    from molt.gpu import Buffer, kernel, to_device, from_device, alloc
    from molt.gpu import thread_id, block_id, block_dim, grid_dim, barrier
    assert callable(kernel)
    assert callable(to_device)
    assert callable(from_device)
    assert callable(alloc)


def test_buffer_generic_subscription_is_runtime_stable():
    from molt.gpu import Buffer

    assert Buffer[float] is Buffer
    assert Buffer[int] is Buffer


def test_buffer_roundtrip_float():
    from molt.gpu import to_device, from_device
    data = [1.0, 2.0, 3.0, 4.0]
    buf = to_device(data)
    assert buf.size == 4
    assert buf.nbytes == 32
    assert buf.element_type is float
    assert from_device(buf) == data


def test_buffer_roundtrip_f32_array():
    import array
    from molt.gpu import to_device, from_device

    data = array.array("f", [1.25, 2.5, 3.75])
    buf = to_device(data)

    assert buf.element_type is float
    assert buf.format_char == "f"
    assert buf.itemsize == 4
    assert buf.nbytes == 12
    assert from_device(buf) == [1.25, 2.5, 3.75]
    assert buf[1] == 2.5


def test_buffer_roundtrip_int():
    from molt.gpu import to_device, from_device
    data = [10, 20, 30]
    buf = to_device(data)
    assert buf.size == 3
    assert buf.element_type is int
    assert from_device(buf) == data


def test_buffer_indexing():
    from molt.gpu import to_device
    buf = to_device([10.0, 20.0, 30.0])
    assert buf[0] == 10.0
    assert buf[1] == 20.0
    assert buf[2] == 30.0


def test_buffer_setitem():
    from molt.gpu import alloc
    buf = alloc(3, float)
    buf[0] = 42.0
    buf[1] = 99.0
    assert buf[0] == 42.0
    assert buf[1] == 99.0
    assert buf[2] == 0.0


def test_alloc_zeros():
    from molt.gpu import alloc, from_device
    buf = alloc(5, float)
    assert from_device(buf) == [0.0, 0.0, 0.0, 0.0, 0.0]


# ── Kernel simulation ───────────────────────────────────────────────────────

def test_kernel_simulation():
    """Kernel decorator + simulated sequential execution.

    IMPORTANT: kernel body must use gpu.thread_id() (module-qualified),
    NOT bare thread_id(), because the simulation monkey-patches the module attr.
    """
    import molt.gpu as gpu

    @gpu.kernel
    def vector_add(a, b, c, n):
        tid = gpu.thread_id()
        if tid < n:
            c[tid] = a[tid] + b[tid]

    a = gpu.to_device([1.0, 2.0, 3.0, 4.0])
    b = gpu.to_device([10.0, 20.0, 30.0, 40.0])
    c = gpu.alloc(4, float)
    vector_add[1, 4](a, b, c, 4)
    result = gpu.from_device(c)
    assert result == [11.0, 22.0, 33.0, 44.0], f"Got {result}"


def test_kernel_scalar_multiply():
    import molt.gpu as gpu

    @gpu.kernel
    def scale(a, out, factor, n):
        tid = gpu.thread_id()
        if tid < n:
            out[tid] = a[tid] * factor

    # NOTE: factor is a plain Python float, not a Buffer
    a = gpu.to_device([2.0, 4.0, 6.0])
    out = gpu.alloc(3, float)
    scale[1, 3](a, out, 3.0, 3)
    result = gpu.from_device(out)
    assert result == [6.0, 12.0, 18.0], f"Got {result}"


# ── Tensor ───────────────────────────────────────────────────────────────────

def test_tensor_create_and_shape():
    from molt.gpu.tensor import Tensor
    t = Tensor([[1, 2, 3], [4, 5, 6]])
    assert t.shape == (2, 3)
    assert t.ndim == 2
    assert t.size == 6


def test_tensor_matmul():
    from molt.gpu.tensor import Tensor
    a = Tensor([[1, 2], [3, 4]])
    b = Tensor([[5, 6], [7, 8]])
    c = a @ b
    assert c.to_list() == [[19.0, 22.0], [43.0, 50.0]]


def test_tensor_linear_weight_layout_matches_transposed_matmul():
    from molt.gpu.tensor import Tensor

    x = Tensor([[1.0, 2.0], [3.0, 4.0]])
    weight = Tensor([[5.0, 6.0], [7.0, 8.0], [9.0, 10.0]])

    expected = x @ weight.transpose()
    got = x.linear(weight)

    assert got.shape == (2, 3)
    assert got.to_list() == expected.to_list()


def test_tensor_linear_preserves_f32_output_layout():
    import array
    from molt.gpu import to_device
    from molt.gpu.tensor import Tensor

    x = Tensor(to_device(array.array("f", [1.0, 2.0, 3.0, 4.0])), shape=(2, 2))
    weight = Tensor(
        to_device(array.array("f", [5.0, 6.0, 7.0, 8.0, 9.0, 10.0])),
        shape=(3, 2),
    )

    out = x.linear(weight)

    assert out.shape == (2, 3)
    assert out.to_list() == [[17.0, 23.0, 29.0], [39.0, 53.0, 67.0]]
    assert out._buf.format_char == "f"
    assert out._buf.itemsize == 4


def test_tensor_take_rows_preserves_values_and_f32_buffer_layout():
    import array
    from molt.gpu import to_device
    from molt.gpu.tensor import Tensor

    weight = Tensor(to_device(array.array("f", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0])), shape=(3, 2))

    out = weight.take_rows([2, 0])

    assert out.shape == (2, 2)
    assert out.to_list() == [[5.0, 6.0], [1.0, 2.0]]
    assert out._buf.format_char == "f"
    assert out._buf.itemsize == 4


def test_tensor_indexing_preserves_subtensor_buffer_layout():
    import array
    from molt.gpu import to_device
    from molt.gpu.tensor import Tensor

    tensor = Tensor(
        to_device(array.array("f", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
        shape=(3, 2),
    )

    row = tensor[1]

    assert row.shape == (2,)
    assert row.to_list() == [3.0, 4.0]
    assert row._buf.format_char == "f"
    assert row._buf.itemsize == 4


def test_tensor_batched_matmul_preserves_all_leading_dims():
    from molt.gpu.tensor import Tensor

    a = Tensor(list(range(1, 1 + 1 * 2 * 3 * 4)), shape=(1, 2, 3, 4))
    b = Tensor(list(range(1, 1 + 1 * 2 * 4 * 2)), shape=(1, 2, 4, 2))

    out = a @ b

    assert out.shape == (1, 2, 3, 2)
    assert out.to_list() == [
        [
            [[50.0, 60.0], [114.0, 140.0], [178.0, 220.0]],
            [[706.0, 764.0], [898.0, 972.0], [1090.0, 1180.0]],
        ]
    ]


def test_tensor_elementwise():
    from molt.gpu.tensor import Tensor
    a = Tensor([1.0, 2.0, 3.0])
    b = Tensor([4.0, 5.0, 6.0])
    assert (a + b).to_list() == [5.0, 7.0, 9.0]
    assert (a * 2).to_list() == [2.0, 4.0, 6.0]
    assert (a - b).to_list() == [-3.0, -3.0, -3.0]


def test_tensor_broadcast_leading_singletons():
    from molt.gpu.tensor import Tensor

    a = Tensor(list(range(1, 1 + 2 * 2 * 2)), shape=(2, 2, 2))
    b = Tensor([10.0, 20.0], shape=(1, 1, 1, 2))

    c = a + b
    assert c.shape == (1, 2, 2, 2)
    assert c.to_list() == [
        [
            [[11.0, 22.0], [13.0, 24.0]],
            [[15.0, 26.0], [17.0, 28.0]],
        ]
    ]


def test_tensor_reshape():
    from molt.gpu.tensor import Tensor
    t = Tensor([1, 2, 3, 4, 5, 6], shape=(6,))
    t2 = t.reshape(2, 3)
    assert t2.shape == (2, 3)
    assert t2.to_list() == [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]


def test_tensor_transpose():
    from molt.gpu.tensor import Tensor
    t = Tensor([[1, 2, 3], [4, 5, 6]])
    tt = t.T
    assert tt.shape == (3, 2)
    assert tt.to_list() == [[1.0, 4.0], [2.0, 5.0], [3.0, 6.0]]


def test_tensor_take_rows_gathers_axis0_slices():
    from molt.gpu.tensor import Tensor

    t = Tensor(
        [
            [1.0, 2.0, 3.0],
            [4.0, 5.0, 6.0],
            [7.0, 8.0, 9.0],
        ]
    )

    out = t.take_rows(Tensor([[2, 0], [1, 2]]))

    assert out.shape == (2, 2, 3)
    assert out.to_list() == [
        [[7.0, 8.0, 9.0], [1.0, 2.0, 3.0]],
        [[4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
    ]


def test_tensor_take_rows_rejects_out_of_range_indices():
    from molt.gpu.tensor import Tensor

    t = Tensor([[1.0, 2.0], [3.0, 4.0]])

    with pytest.raises(IndexError, match="out of range"):
        t.take_rows(Tensor([2]))


def test_tensor_permute_4d_common_orders():
    from molt.gpu.tensor import Tensor

    t = Tensor(list(range(1, 17)), shape=(1, 2, 2, 4))

    shsd = t.permute(0, 2, 1, 3)
    assert shsd.shape == (1, 2, 2, 4)
    assert shsd.to_list() == [
        [
            [[1.0, 2.0, 3.0, 4.0], [9.0, 10.0, 11.0, 12.0]],
            [[5.0, 6.0, 7.0, 8.0], [13.0, 14.0, 15.0, 16.0]],
        ]
    ]

    ssdh = t.permute(0, 1, 3, 2)
    assert ssdh.shape == (1, 2, 4, 2)
    assert ssdh.to_list() == [
        [
            [[1.0, 5.0], [2.0, 6.0], [3.0, 7.0], [4.0, 8.0]],
            [[9.0, 13.0], [10.0, 14.0], [11.0, 15.0], [12.0, 16.0]],
        ]
    ]


def test_tensor_reductions():
    from molt.gpu.tensor import Tensor
    t = Tensor([1.0, 2.0, 3.0, 4.0])
    assert t.sum().item() == 10.0
    assert t.mean().item() == 2.5
    assert t.max().item() == 4.0
    assert t.min().item() == 1.0


def test_tensor_mean_keepdim_preserves_reduced_axis():
    from molt.gpu.tensor import Tensor

    t = Tensor([1.0, 2.0, 3.0, 4.0], shape=(1, 2, 2))
    m = t.mean(axis=-1, keepdim=True)

    assert m.shape == (1, 2, 1)
    assert m.to_list() == [[[1.5], [3.5]]]


def test_tensor_activations():
    from molt.gpu.tensor import Tensor
    t = Tensor([-1.0, 0.0, 1.0, 2.0])
    relu = t.relu().to_list()
    assert relu == [0.0, 0.0, 1.0, 2.0]
    sig = t.sigmoid().to_list()
    assert 0.26 < sig[0] < 0.28  # sigmoid(-1) ~ 0.2689
    assert sig[1] == 0.5          # sigmoid(0) = 0.5
    sm = t.softmax().to_list()
    assert abs(sum(sm) - 1.0) < 1e-6  # softmax sums to 1


def test_tensor_constructors():
    from molt.gpu.tensor import zeros, ones, randn
    z = zeros(3, 4)
    assert z.shape == (3, 4)
    assert z.sum().item() == 0.0
    o = ones(2, 2)
    assert o.sum().item() == 4.0
    r = randn(10, seed=42)
    assert r.shape == (10,)


# ── DataFrame ────────────────────────────────────────────────────────────────

def test_dataframe_create():
    from molt.gpu.dataframe import DataFrame
    df = DataFrame({
        "price": [10.5, 20.3, 15.7],
        "name": ["a", "b", "c"],
    })
    assert df.shape == (3, 2)
    assert df.columns == ["price", "name"]


def test_dataframe_filter():
    from molt.gpu.dataframe import DataFrame
    df = DataFrame({"x": [1.0, 2.0, 3.0, 4.0, 5.0]})
    filtered = df.filter(df["x"] > 3.0)
    assert len(filtered) == 2


def test_dataframe_groupby():
    from molt.gpu.dataframe import DataFrame
    df = DataFrame({
        "cat": ["A", "B", "A", "B"],
        "val": [10.0, 20.0, 30.0, 40.0],
    })
    result = df.group_by("cat").agg(total=("val", "sum"))
    d = result.to_dict()
    # Order may vary
    assert set(d["cat"]) == {"A", "B"}
    totals = dict(zip(d["cat"], d["total"]))
    assert totals["A"] == 40.0
    assert totals["B"] == 60.0


def test_dataframe_sort():
    from molt.gpu.dataframe import DataFrame
    df = DataFrame({"x": [3.0, 1.0, 2.0]})
    sorted_df = df.sort("x")
    assert sorted_df["x"].to_list() == [1.0, 2.0, 3.0]


# ── Ops ──────────────────────────────────────────────────────────────────────

def test_ops_reduce():
    from molt.gpu import to_device
    from molt.gpu.ops import reduce
    buf = to_device([1.0, 2.0, 3.0, 4.0, 5.0])
    assert reduce(buf, "sum") == 15.0
    assert reduce(buf, "max") == 5.0
    assert reduce(buf, "min") == 1.0
    assert reduce(buf, "mean") == 3.0


def test_ops_map():
    from molt.gpu import to_device, from_device
    from molt.gpu.ops import map
    buf = to_device([1.0, 2.0, 3.0])
    result = map(lambda x: x * 2.0, buf)
    assert from_device(result) == [2.0, 4.0, 6.0]


def test_ops_filter():
    from molt.gpu import to_device, from_device
    from molt.gpu.ops import filter
    buf = to_device([1.0, 2.0, 3.0, 4.0, 5.0])
    result = filter(lambda x: x > 3.0, buf)
    assert from_device(result)[:result.size] == [4.0, 5.0]


def test_ops_scan():
    from molt.gpu import to_device, from_device
    from molt.gpu.ops import scan
    buf = to_device([1.0, 2.0, 3.0, 4.0])
    result = scan(buf, "sum")
    assert from_device(result) == [1.0, 3.0, 6.0, 10.0]


def test_ops_dot():
    from molt.gpu import to_device
    from molt.gpu.ops import dot
    a = to_device([1.0, 2.0, 3.0])
    b = to_device([4.0, 5.0, 6.0])
    assert dot(a, b) == 32.0


def test_ops_norm():
    from molt.gpu import to_device
    from molt.gpu.ops import norm
    buf = to_device([3.0, 4.0])
    assert norm(buf, 2) == 5.0


# ── Sub-module imports ───────────────────────────────────────────────────────

def test_submodule_nn():
    from molt.gpu.nn import Linear, ReLU, Sequential, LayerNorm, Conv2d, Embedding
    from molt.gpu.tensor import Tensor, randn
    linear = Linear(4, 2)
    x = randn(1, 4, seed=1)
    out = linear(x)
    assert out.shape == (1, 2)


def test_embedding_lookup_avoids_full_weight_materialization(monkeypatch):
    from molt.gpu.nn import Embedding
    from molt.gpu.tensor import Tensor

    emb = Embedding(3, 2)
    emb.load_weights(Tensor([[1.0, 2.0], [3.0, 4.0], [5.0, 6.0]]))
    monkeypatch.setattr(
        emb.weight,
        "_data_list",
        lambda: (_ for _ in ()).throw(
            AssertionError("embedding lookup should not flatten full weight")
        ),
    )

    out = emb(Tensor([2, 0]))

    assert out.shape == (2, 2)
    assert out.to_list() == [[5.0, 6.0], [1.0, 2.0]]


def test_submodule_transformer():
    from molt.gpu.transformer import TransformerBlock


def test_submodule_quantize():
    from molt.gpu.quantize import QuantizedTensor, QuantizedLinear


def test_submodule_gguf():
    from molt.gpu.gguf import load_gguf, GGUFModel


def test_submodule_hub():
    from molt.gpu.hub import download_model, list_files


def test_submodule_interop():
    from molt.gpu.interop import load_safetensors


def test_load_safetensors_materializes_tensors_on_demand(tmp_path, monkeypatch):
    import molt.gpu.interop as interop
    from molt.gpu import from_device

    safetensors_path = tmp_path / "weights.safetensors"
    _write_safetensors_fixture(safetensors_path)

    materialized: list[tuple[list[float], tuple[int, ...]]] = []

    class FakeTensor:
        def __init__(self, data, shape=None, dtype=float):
            if hasattr(data, "element_type") and hasattr(data, "size"):
                values = from_device(data)
            else:
                values = list(data)
            materialized.append((values, tuple(shape or ())))
            self.shape = tuple(shape or ())
            self.dtype = dtype

    monkeypatch.setattr(interop, "Tensor", FakeTensor)

    weights = interop.load_safetensors(str(safetensors_path))

    assert len(weights) == 2
    assert materialized == []

    first = weights["t0"]
    assert isinstance(first, FakeTensor)
    assert materialized == [([1.0, 2.0], (2,))]
    assert weights["t0"] is first

    second = weights.get("t1")
    assert isinstance(second, FakeTensor)
    assert materialized == [
        ([1.0, 2.0], (2,)),
        ([3.0, 4.0, 5.0, 6.0], (2, 2)),
    ]
    assert weights.get("missing", "fallback") == "fallback"


def test_load_safetensors_f32_entries_stay_f32_buffer_backed(tmp_path):
    from molt.gpu.interop import load_safetensors
    from molt.gpu import from_device

    safetensors_path = tmp_path / "weights_f32.safetensors"
    _write_safetensors_fixture(safetensors_path)

    weights = load_safetensors(str(safetensors_path))
    tensor = weights["t0"]

    assert tensor._buf.format_char == "f"
    assert tensor._buf.itemsize == 4
    assert from_device(tensor._buf) == [1.0, 2.0]


def test_submodule_numpy_io():
    from molt.gpu.numpy_io import load_numpy, load_npz


def test_submodule_fusion():
    from molt.gpu.fusion import FusedPipeline, fused_map_reduce


def test_submodule_distributed():
    from molt.gpu.distributed import Cluster, Worker


def test_submodule_generate():
    from molt.gpu.generate import greedy_decode, top_k_sample


if __name__ == "__main__":
    # Run all tests manually
    import traceback
    passed = 0
    failed = 0
    for name, func in sorted(globals().items()):
        if name.startswith("test_") and callable(func):
            try:
                func()
                print(f"  PASS  {name}")
                passed += 1
            except Exception as e:
                print(f"  FAIL  {name}: {e}")
                traceback.print_exc()
                failed += 1
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)
