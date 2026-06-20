from __future__ import annotations

import pytest

from tests.helpers.tinygrad_stdlib_loader import tinygrad_stdlib_context


def test_tensor_bytes_constructor_uses_uint8_public_dtype_and_values() -> None:
    with tinygrad_stdlib_context() as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        tensor = Tensor(b"\x00\x01\xff")

        assert tensor.dtype is dtypes.uint8
        assert tensor.shape == (3,)
        assert tensor.tolist() == [0, 1, 255]


def test_tensor_uint16_constructor_emits_exact_raw_upload() -> None:
    calls: dict[str, object] = {}

    def create_raw(data, data_len, dtype_code, shape):
        calls["data"] = bytes(data)
        calls["data_len"] = data_len
        calls["dtype_code"] = dtype_code
        calls["shape"] = tuple(shape)
        return 42

    with tinygrad_stdlib_context(
        intrinsics={"molt_gpu_prim_create_tensor_raw": create_raw}
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        tensor = Tensor([1, 258, 65535], dtype=dtypes.uint16)

        assert tensor.lazydata.handle == 42
        assert calls == {
            "data": b"\x01\x00\x02\x01\xff\xff",
            "data_len": 6,
            "dtype_code": dtypes.uint16.code,
            "shape": (3,),
        }
        assert tensor.tolist() == [1, 258, 65535]


def test_handle_only_int64_tensor_reads_back_exact_raw_values() -> None:
    payload = b"".join(
        value.to_bytes(8, "little", signed=True) for value in (-1, 0, 2**40)
    )

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 77
        assert out_len == len(payload)
        out[: len(payload)] = payload
        return len(payload)

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 4,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        LazyBuffer = modules["lazy"].LazyBuffer
        dtypes = modules["dtypes"].dtypes

        tensor = Tensor(LazyBuffer(None, dtypes.int64, (3,), handle=77))

        assert tensor.tolist() == [-1, 0, 2**40]


def test_zeros_uint64_uses_typed_zero_intrinsic_and_raw_readback() -> None:
    calls: dict[str, object] = {}

    def zeros_dtype(dtype_code, shape):
        calls["dtype_code"] = dtype_code
        calls["shape"] = tuple(shape)
        return 88

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 88
        out[:out_len] = b"\x00" * out_len
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_zeros_dtype": zeros_dtype,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 8,
            "molt_gpu_prim_nbytes": lambda handle: 32,
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        tensor = Tensor.zeros((2, 2), dtype=dtypes.uint64)

        assert tensor.lazydata.handle == 88
        assert calls == {"dtype_code": dtypes.uint64.code, "shape": (2, 2)}
        assert tensor.tolist() == [[0, 0], [0, 0]]


def test_int32_binary_add_carries_runtime_handle_to_raw_readback() -> None:
    calls: list[tuple] = []
    next_handle = {"value": 100}

    def create_raw(data, data_len, dtype_code, shape):
        handle = next_handle["value"]
        next_handle["value"] += 1
        calls.append(
            ("create_raw", bytes(data), data_len, dtype_code, tuple(shape), handle)
        )
        return handle

    def binary(op_code, lhs_handle, rhs_handle):
        calls.append(("binary", op_code, lhs_handle, rhs_handle))
        return 200

    payload = b"".join(value.to_bytes(4, "little", signed=True) for value in (5, 3, -1))

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 200
        assert dtype_code == 3
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_binary": binary,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = Tensor([1, -2, 7], dtype=dtypes.int32) + Tensor(
            [4, 5, -8], dtype=dtypes.int32
        )

        assert result.lazydata.handle == 200
        assert result.lazydata._data is None
        assert ("binary", 0, 100, 101) in calls
        assert result.tolist() == [5, 3, -1]


def test_runtime_cast_keeps_target_dtype_without_host_materialization() -> None:
    calls: list[tuple] = []

    def create_raw(data, data_len, dtype_code, shape):
        calls.append(("create_raw", dtype_code, tuple(shape)))
        return 300

    def cast(op_code, src_handle, dst_dtype_code):
        calls.append(("cast", op_code, src_handle, dst_dtype_code))
        return 301

    payload = b"".join(value.to_bytes(4, "little", signed=True) for value in (1, -2, 0))

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 301
        assert dtype_code == 3
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_cast": cast,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = Tensor([1.25, -2.75, 0.0], dtype=dtypes.float32).cast(dtypes.int32)

        assert result.dtype is dtypes.int32
        assert result.lazydata.handle == 301
        assert result.lazydata._data is None
        assert ("cast", 22, 300, dtypes.int32.code) in calls
        assert result.tolist() == [1, -2, 0]


def test_runtime_unary_and_explicit_axis_reduce_use_primitive_handles() -> None:
    calls: list[tuple] = []

    def create_raw(data, data_len, dtype_code, shape):
        calls.append(("create_raw", dtype_code, tuple(shape)))
        return 400

    def unary(op_code, src_handle):
        calls.append(("unary", op_code, src_handle))
        return 401

    def reduce(op_code, src_handle, axis):
        calls.append(("reduce", op_code, src_handle, axis))
        return 402

    payload = b"".join(value.to_bytes(4, "little", signed=True) for value in (-3, -7))

    def read_raw(handle, dtype_code, out, out_len):
        assert handle in (401, 402)
        out[:out_len] = payload[:out_len]
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_unary": unary,
            "molt_gpu_prim_reduce": reduce,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        negated = Tensor([3, 7], dtype=dtypes.int32).neg()
        reduced = negated.sum(axis=0)

        assert negated.lazydata.handle == 401
        assert negated.lazydata._data is None
        assert reduced.lazydata.handle == 402
        assert ("unary", 5, 400) in calls
        assert ("reduce", 24, 401, 0) in calls


def test_runtime_movement_views_use_primitive_handles_without_materializing_source() -> (
    None
):
    calls: list[tuple] = []

    def create_raw(_data, _data_len, dtype_code, shape):
        calls.append(("create_raw", dtype_code, tuple(shape)))
        return 700

    def reshape(handle, shape):
        calls.append(("reshape", handle, tuple(shape)))
        return 701

    def expand(handle, shape):
        calls.append(("expand", handle, tuple(shape)))
        return 702

    def permute(handle, order):
        calls.append(("permute", handle, tuple(order)))
        return 703

    def contiguous(handle):
        calls.append(("contiguous", handle))
        return 704

    payload = b"".join(value.to_bytes(4, "little", signed=True) for value in range(12))

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 704
        assert dtype_code == 3
        assert out_len == len(payload)
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_reshape": reshape,
            "molt_gpu_prim_expand": expand,
            "molt_gpu_prim_permute": permute,
            "molt_gpu_prim_contiguous": contiguous,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = (
            Tensor([1, 2, 3, 4], dtype=dtypes.int32)
            .reshape(1, 4)
            .expand(3, 4)
            .permute(1, 0)
            .contiguous()
        )

        assert result.shape == (4, 3)
        assert result.lazydata.handle == 704
        assert result.lazydata._data is None
        assert calls == [
            ("create_raw", dtypes.int32.code, (4,)),
            ("reshape", 700, (1, 4)),
            ("expand", 701, (3, 4)),
            ("permute", 702, (1, 0)),
            ("contiguous", 703),
        ]
        assert result.tolist() == [[0, 1, 2], [3, 4, 5], [6, 7, 8], [9, 10, 11]]


def test_runtime_pad_shrink_flip_use_primitive_handles_without_materializing_source() -> (
    None
):
    calls: list[tuple] = []

    def create_raw(_data, _data_len, dtype_code, shape):
        calls.append(("create_raw", dtype_code, tuple(shape)))
        return 900

    def pad(handle, padding):
        calls.append(("pad", handle, tuple(padding)))
        return 901

    def shrink(handle, bounds):
        calls.append(("shrink", handle, tuple(bounds)))
        return 902

    def flip(handle, axis):
        calls.append(("flip", handle, axis))
        return 903

    payload = b"".join(
        value.to_bytes(4, "little", signed=True) for value in (4, 3, 2, 1)
    )

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 903
        assert dtype_code == 3
        assert out_len == len(payload)
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_pad": pad,
            "molt_gpu_prim_shrink": shrink,
            "molt_gpu_prim_flip": flip,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = (
            Tensor([1, 2, 3, 4], dtype=dtypes.int32)
            .pad(((1, 1),))
            .shrink(((1, 5),))
            .flip(0)
        )

        assert result.shape == (4,)
        assert result.lazydata.handle == 903
        assert result.lazydata._data is None
        assert calls == [
            ("create_raw", dtypes.int32.code, (4,)),
            ("pad", 900, (1, 1)),
            ("shrink", 901, (1, 5)),
            ("flip", 902, 0),
        ]
        assert result.tolist() == [4, 3, 2, 1]


def test_runtime_nonzero_pad_on_handle_backed_tensor_raises() -> None:
    def create_raw(_data, _data_len, _dtype_code, _shape):
        return 910

    with tinygrad_stdlib_context(
        intrinsics={"molt_gpu_prim_create_tensor_raw": create_raw}
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        with pytest.raises(RuntimeError, match="zero padding only"):
            Tensor([1, 2], dtype=dtypes.int32).pad(((1, 1),), value=9.0)


def test_runtime_matmul_composes_movement_binary_reduce_handles() -> None:
    calls: list[tuple] = []
    next_create = {"handle": 800}

    def create_raw(_data, _data_len, dtype_code, shape):
        handle = next_create["handle"]
        next_create["handle"] += 1
        calls.append(("create_raw", dtype_code, tuple(shape), handle))
        return handle

    movement_handles = iter([802, 803, 804, 805, 808])

    def reshape(handle, shape):
        out = next(movement_handles)
        calls.append(("reshape", handle, tuple(shape), out))
        return out

    def expand(handle, shape):
        out = next(movement_handles)
        calls.append(("expand", handle, tuple(shape), out))
        return out

    def binary(op_code, lhs_handle, rhs_handle):
        calls.append(("binary", op_code, lhs_handle, rhs_handle))
        return 806

    def reduce(op_code, src_handle, axis):
        calls.append(("reduce", op_code, src_handle, axis))
        return 807

    payload = b"".join(
        value.to_bytes(4, "little", signed=True) for value in (19, 22, 43, 50)
    )

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 808
        assert dtype_code == 3
        assert out_len == len(payload)
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_reshape": reshape,
            "molt_gpu_prim_expand": expand,
            "molt_gpu_prim_binary": binary,
            "molt_gpu_prim_reduce": reduce,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = Tensor([[1, 2], [3, 4]], dtype=dtypes.int32) @ Tensor(
            [[5, 6], [7, 8]], dtype=dtypes.int32
        )

        assert result.shape == (2, 2)
        assert result.lazydata.handle == 808
        assert result.lazydata._data is None
        assert calls == [
            ("create_raw", dtypes.int32.code, (2, 2), 800),
            ("create_raw", dtypes.int32.code, (2, 2), 801),
            ("reshape", 800, (2, 2, 1), 802),
            ("expand", 802, (2, 2, 2), 803),
            ("reshape", 801, (1, 2, 2), 804),
            ("expand", 804, (2, 2, 2), 805),
            ("binary", 2, 803, 805),
            ("reduce", 24, 806, 1),
            ("reshape", 807, (2, 2), 808),
        ]
        assert result.tolist() == [[19, 22], [43, 50]]


def test_axis_none_reduce_chains_runtime_axes_and_reshapes_public_scalar() -> None:
    calls: list[tuple] = []

    def create_raw(_data, _data_len, dtype_code, shape):
        calls.append(("create_raw", dtype_code, tuple(shape)))
        return 920

    def reduce_all(op_code, handle):
        calls.append(("reduce_all", op_code, handle))
        return 923

    payload = (10).to_bytes(4, "little", signed=True)

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 923
        assert dtype_code == 3
        assert out_len == len(payload)
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_reduce_all": reduce_all,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = Tensor([[1, 2], [3, 4]], dtype=dtypes.int32).sum(axis=None)

        assert result.lazydata.handle == 923
        assert result.lazydata._data is None
        assert calls == [
            ("create_raw", dtypes.int32.code, (2, 2)),
            ("reduce_all", 24, 920),
        ]
        assert result.tolist() == [10]


def test_runtime_explicit_axis_reduce_keeps_broadcastable_runtime_shape() -> None:
    calls: list[tuple] = []

    def create_raw(_data, _data_len, dtype_code, shape):
        calls.append(("create_raw", dtype_code, tuple(shape)))
        return 930

    def reduce(op_code, handle, axis):
        calls.append(("reduce", op_code, handle, axis))
        return 931

    def expand(handle, shape):
        calls.append(("expand", handle, tuple(shape)))
        return 932

    payload = b"".join(
        value.to_bytes(4, "little", signed=True) for value in (3, 3, 7, 7)
    )

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 932
        assert dtype_code == 3
        assert out_len == len(payload)
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_reduce": reduce,
            "molt_gpu_prim_expand": expand,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = (
            Tensor([[1, 2], [3, 4]], dtype=dtypes.int32)
            .sum(axis=1)
            ._broadcast_to((2, 2))
        )

        assert result.shape == (2, 2)
        assert result.lazydata.handle == 932
        assert result.lazydata._data is None
        assert calls == [
            ("create_raw", dtypes.int32.code, (2, 2)),
            ("reduce", 24, 930, 1),
            ("expand", 931, (2, 2)),
        ]
        assert result.tolist() == [[3, 3], [7, 7]]


def test_runtime_binary_stays_reference_when_lhs_shape_would_misstate_broadcast() -> (
    None
):
    def binary(*_args):
        raise AssertionError(
            "runtime binary must not own lhs-shape-mismatched broadcast"
        )

    with tinygrad_stdlib_context(
        intrinsics={"molt_gpu_prim_binary": binary}
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = 10 - Tensor([1, 2, 3], dtype=dtypes.int32)

        assert result.shape == (3,)
        assert result.lazydata.handle is None
        assert result.tolist() == [9, 8, 7]


def test_where_matches_upstream_bool_cast_promotion_descriptor_and_broadcast() -> None:
    with tinygrad_stdlib_context() as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        scalar_result = Tensor([2, 0, -3], dtype=dtypes.int32).where(1, 3)

        assert scalar_result.dtype is dtypes.int32
        assert scalar_result.shape == (3,)
        assert scalar_result.tolist() == [1, 3, 1]

        descriptor_result = Tensor.where(
            Tensor([0, 1], dtype=dtypes.int32),
            Tensor([7], dtype=dtypes.int32),
            Tensor([2, 3], dtype=dtypes.int64),
        )

        assert descriptor_result.dtype is dtypes.int64
        assert descriptor_result.shape == (2,)
        assert descriptor_result.tolist() == [2, 7]


def test_runtime_where_uses_ternary_handle_without_materializing_sources() -> None:
    calls: list[tuple] = []
    next_handle = {"value": 940}

    def create_raw(_data, _data_len, dtype_code, shape):
        handle = next_handle["value"]
        next_handle["value"] += 1
        calls.append(("create_raw", dtype_code, tuple(shape), handle))
        return handle

    def cast(op_code, handle, dst_dtype_code):
        calls.append(("cast", op_code, handle, dst_dtype_code))
        return 943

    def ternary(op_code, cond_handle, true_handle, false_handle):
        calls.append(("ternary", op_code, cond_handle, true_handle, false_handle))
        return 944

    payload = b"".join(
        value.to_bytes(4, "little", signed=True) for value in (10, 2, 30)
    )

    def read_raw(handle, dtype_code, out, out_len):
        assert handle == 944
        assert dtype_code == 3
        assert out_len == len(payload)
        out[:out_len] = payload
        return out_len

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_cast": cast,
            "molt_gpu_prim_ternary": ternary,
            "molt_gpu_prim_realize": lambda handle: 0,
            "molt_gpu_prim_dtype": lambda handle: 3,
            "molt_gpu_prim_nbytes": lambda handle: len(payload),
            "molt_gpu_prim_read_data_raw": read_raw,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        result = Tensor([1, 0, 1], dtype=dtypes.int32).where(
            Tensor([10, 20, 30], dtype=dtypes.int32),
            Tensor([1, 2, 3], dtype=dtypes.int32),
        )

        assert result.shape == (3,)
        assert result.dtype is dtypes.int32
        assert result.lazydata.handle == 944
        assert result.lazydata._data is None
        assert calls == [
            ("create_raw", dtypes.int32.code, (3,), 940),
            ("create_raw", dtypes.int32.code, (3,), 941),
            ("create_raw", dtypes.int32.code, (3,), 942),
            ("cast", 22, 940, dtypes.bool_.code),
            ("ternary", 21, 943, 941, 942),
        ]
        assert result.tolist() == [10, 2, 30]


def test_runtime_where_fails_closed_when_ternary_intrinsic_rejects_handle_graph() -> (
    None
):
    next_handle = {"value": 950}

    def create_raw(_data, _data_len, _dtype_code, _shape):
        handle = next_handle["value"]
        next_handle["value"] += 1
        return handle

    with tinygrad_stdlib_context(
        intrinsics={
            "molt_gpu_prim_create_tensor_raw": create_raw,
            "molt_gpu_prim_cast": lambda _op_code, _handle, _dst_dtype_code: 953,
            "molt_gpu_prim_ternary": lambda *_args: (1 << 64) - 1,
        }
    ) as modules:
        Tensor = modules["tensor"].Tensor
        dtypes = modules["dtypes"].dtypes

        with pytest.raises(RuntimeError, match="molt GPU runtime where failed"):
            Tensor([1, 0, 1], dtype=dtypes.int32).where(
                Tensor([10, 20, 30], dtype=dtypes.int32),
                Tensor([1, 2, 3], dtype=dtypes.int32),
            )
