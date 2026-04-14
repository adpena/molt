"""KV-cache backends for projected attention tensors."""

from __future__ import annotations

import math
import _intrinsics as _molt_intrinsics

from . import Buffer
from .tensor import Tensor, tensor_permute_dims, tensor_softmax_last_axis
from .turboquant import TurboQuantCodec


def _load_optional_intrinsic(name: str):
    loader = getattr(_molt_intrinsics, "load_intrinsic", None)
    if callable(loader):
        return loader(name)
    require = getattr(_molt_intrinsics, "require_intrinsic", None)
    if callable(require):
        try:
            return require(name)
        except RuntimeError:
            return None
    return None


_MOLT_GPU_TURBOQUANT_ATTENTION_PACKED = _load_optional_intrinsic(
    "molt_gpu_turboquant_attention_packed"
)


def _validate_projected_tensor(name: str, tensor: Tensor) -> None:
    if not isinstance(tensor, Tensor):
        raise TypeError(f"{name} must be a Tensor")
    if tensor.ndim != 4:
        raise ValueError(
            f"{name} must have shape (batch, heads, seq, head_dim), got {tensor.shape}"
        )


def _broadcast_mask_row(mask: Tensor, batch_index: int, head_index: int, query_index: int):
    if mask.ndim != 4:
        raise ValueError(f"attention mask must be 4D for KV cache attention, got {mask.shape}")
    b_index = batch_index if mask.shape[0] > 1 else 0
    h_index = head_index if mask.shape[1] > 1 else 0
    q_index = query_index if mask.shape[2] > 1 else 0
    return mask[b_index, h_index, q_index, :].to_list()


def _expanded_mask_row(mask_row, width: int) -> list[float]:
    if len(mask_row) == width:
        return [float(value) for value in mask_row]
    if len(mask_row) == 1:
        value = float(mask_row[0])
        return [value] * width
    raise ValueError(
        f"attention mask row width {len(mask_row)} does not match cached key width {width}"
    )


def _kv_head_index(query_heads: int, kv_heads: int, query_head_index: int) -> int:
    if query_heads == kv_heads:
        return query_head_index
    if query_heads < kv_heads or query_heads % kv_heads != 0:
        raise ValueError(
            f"query head count {query_heads} is incompatible with kv head count {kv_heads}"
        )
    group_size = query_heads // kv_heads
    return query_head_index // group_size


def _materialize_projected_chunks(chunks) -> Tensor:
    first = chunks[0]
    batch, heads, _seq, head_dim = first.shape
    format_char = first._buf.format_char
    element_type = first._buf.element_type
    dtype = first._dtype
    itemsize = first._buf.itemsize

    total_seq = 0
    for chunk in chunks:
        if chunk.ndim != 4:
            raise ValueError("KV cache materialization requires rank-4 chunk tensors")
        if chunk.shape[:2] != (batch, heads) or chunk.shape[3] != head_dim:
            raise ValueError("KV cache chunks must share batch/head/head_dim shape")
        if chunk._buf.format_char != format_char or chunk._dtype is not dtype:
            raise ValueError("KV cache chunks must share dtype and buffer format")
        total_seq += chunk.shape[2]

    outer = batch * heads
    row_total_bytes = total_seq * head_dim * itemsize
    out = bytearray(outer * row_total_bytes)
    seq_offset = 0

    for chunk in chunks:
        chunk_seq = chunk.shape[2]
        row_chunk_bytes = chunk_seq * head_dim * itemsize
        src = chunk._buf._data
        dst_row_offset = seq_offset * head_dim * itemsize
        for outer_idx in range(outer):
            src_base = outer_idx * row_chunk_bytes
            dst_base = outer_idx * row_total_bytes + dst_row_offset
            out[dst_base:dst_base + row_chunk_bytes] = src[src_base:src_base + row_chunk_bytes]
        seq_offset += chunk_seq

    out_buf = Buffer(
        out,
        element_type,
        batch * heads * total_seq * head_dim,
        format_char=format_char,
    )
    return Tensor(out_buf, shape=(batch, heads, total_seq, head_dim), dtype=dtype)


class DenseKVCache:
    """Reference dense KV cache for projected multi-head attention tensors."""

    def __init__(self) -> None:
        self._key_chunks = []
        self._value_chunks = []
        self._batch = None
        self._heads = None
        self._head_dim = None
        self._length = 0
        self._prepared_dense_views = {}
        self.key_tensor = None
        self.value_tensor = None

    def __len__(self) -> int:
        return self._length

    def append(self, k: Tensor, v: Tensor) -> None:
        _validate_projected_tensor("k", k)
        _validate_projected_tensor("v", v)
        if k.shape != v.shape:
            raise ValueError(f"k and v must have identical shapes, got {k.shape} and {v.shape}")
        batch, heads, seq, head_dim = k.shape
        if self._batch is None:
            self._batch = batch
            self._heads = heads
            self._head_dim = head_dim
        elif self._batch != batch or self._heads != heads or self._head_dim != head_dim:
            raise ValueError(
                "KV cache append requires matching batch, heads, and head_dim"
            )
        self._key_chunks.append(k)
        self._value_chunks.append(v)
        self._length += seq
        self._prepared_dense_views.clear()
        self.key_tensor = None
        self.value_tensor = None

    def _materialize(self) -> tuple[Tensor, Tensor]:
        if self.key_tensor is not None and self.value_tensor is not None:
            return self.key_tensor, self.value_tensor
        if not self._key_chunks or not self._value_chunks:
            raise RuntimeError("cannot materialize an empty KV cache")
        key = _materialize_projected_chunks(self._key_chunks)
        value = _materialize_projected_chunks(self._value_chunks)
        self.key_tensor = key
        self.value_tensor = value
        self._key_chunks = [key]
        self._value_chunks = [value]
        return key, value

    def attention(self, q: Tensor, *, scale: float, mask: Tensor | None = None) -> Tensor:
        return self._attention_reference(q, mask, scale)

    def _attention_reference(
        self, q: Tensor, mask: Tensor | None, scale: float
    ) -> Tensor:
        _validate_projected_tensor("q", q)
        if not self._key_chunks or not self._value_chunks:
            raise RuntimeError("cannot attend with an empty KV cache")
        key_tensor, value_tensor = self._materialize()
        batch, heads, _query_seq, _head_dim = q.shape
        if batch != self._batch:
            raise ValueError("query batch shape must match the KV cache")
        if heads == self._heads:
            expanded_key = key_tensor
            expanded_value = value_tensor
            expanded_key_t = self._prepared_dense_views.get(("permute", heads))
            if expanded_key_t is None:
                expanded_key_t = tensor_permute_dims(expanded_key, (0, 1, 3, 2))
                self._prepared_dense_views[("permute", heads)] = expanded_key_t
        else:
            prepared = self._prepared_dense_views.get(("gqa", heads))
            if prepared is None:
                repeats = heads // self._heads if self._heads else 0
                if heads < self._heads or heads % self._heads != 0:
                    raise ValueError(
                        f"query head count {heads} is incompatible with kv head count {self._heads}"
                    )
                expanded_key = key_tensor.repeat_axis(1, repeats)
                expanded_value = value_tensor.repeat_axis(1, repeats)
                expanded_key_t = tensor_permute_dims(expanded_key, (0, 1, 3, 2))
                self._prepared_dense_views[("gqa", heads)] = (
                    expanded_key,
                    expanded_value,
                    expanded_key_t,
                )
            else:
                expanded_key, expanded_value, expanded_key_t = prepared

        scores = (q @ expanded_key_t) * scale
        if mask is not None:
            scores = scores + mask
        attn = tensor_softmax_last_axis(scores)
        return attn @ expanded_value

    def truncate(self, length: int) -> None:
        if length < 0:
            raise ValueError("KV cache length must be non-negative")
        if not self._key_chunks or not self._value_chunks:
            if length != 0:
                raise ValueError("cannot truncate an empty KV cache to a non-zero length")
            return
        current = len(self)
        if length > current:
            raise ValueError("cannot extend KV cache via truncate")
        if length == current:
            return
        if length == 0:
            self._key_chunks = []
            self._value_chunks = []
            self._batch = None
            self._heads = None
            self._head_dim = None
            self._length = 0
            self._prepared_dense_views.clear()
            self.key_tensor = None
            self.value_tensor = None
            return
        key_tensor, value_tensor = self._materialize()
        self.key_tensor = key_tensor[:, :, :length, :]
        self.value_tensor = value_tensor[:, :, :length, :]
        self._key_chunks = [self.key_tensor]
        self._value_chunks = [self.value_tensor]
        self._length = length
        self._prepared_dense_views.clear()

    def keys(self):
        return _KVCacheKeyView(self)

    def values(self):
        return _KVCacheValueView(self)


class TurboQuantAttentionKVCache:
    """TurboQuant-backed KV cache for projected multi-head attention tensors."""

    def __init__(self, codec: TurboQuantCodec) -> None:
        self.codec = codec
        self._key_vectors = None
        self._value_vectors = None
        self._batch = None
        self._heads = None
        self._decoded_value_rows = None
        self._runtime_mse_signs = None
        self._runtime_qjl_signs = None
        self._runtime_key_mse_weight_rows = None
        self._runtime_key_residual_sign_rows = None
        self._runtime_key_residual_scale_rows = None
        self._runtime_value_rows = None

    def __len__(self) -> int:
        if self._key_vectors is None:
            return 0
        return len(self._key_vectors[0][0])

    def append(self, k: Tensor, v: Tensor) -> None:
        _validate_projected_tensor("k", k)
        _validate_projected_tensor("v", v)
        if k.shape != v.shape:
            raise ValueError(f"k and v must have identical shapes, got {k.shape} and {v.shape}")
        batch, heads, seq, head_dim = k.shape
        if head_dim != self.codec.dim:
            raise ValueError(
                f"TurboQuant codec dim {self.codec.dim} does not match head_dim {head_dim}"
            )
        if self._key_vectors is None:
            self._batch = batch
            self._heads = heads
            self._key_vectors = [[[] for _ in range(heads)] for _ in range(batch)]
            self._value_vectors = [[[] for _ in range(heads)] for _ in range(batch)]
        elif self._batch != batch or self._heads != heads:
            raise ValueError("KV cache append requires matching batch and head counts")

        k_rows = k.to_list()
        v_rows = v.to_list()
        for batch_index in range(batch):
            for head_index in range(heads):
                for seq_index in range(seq):
                    self._key_vectors[batch_index][head_index].append(
                        self.codec.quantize_prod(k_rows[batch_index][head_index][seq_index])
                    )
                    self._value_vectors[batch_index][head_index].append(
                        self.codec.quantize_prod(v_rows[batch_index][head_index][seq_index])
                    )
        self._decoded_value_rows = None
        self._invalidate_runtime_shadow_rows()

    def attention(self, q: Tensor, *, scale: float, mask: Tensor | None = None) -> Tensor:
        if _MOLT_GPU_TURBOQUANT_ATTENTION_PACKED is not None:
            return _MOLT_GPU_TURBOQUANT_ATTENTION_PACKED(
                q,
                self.keys(),
                self.values(),
                mask,
                scale,
            )
        return self._attention_reference(q, mask, scale)

    def _attention_reference(
        self, q: Tensor, mask: Tensor | None, scale: float
    ) -> Tensor:
        _validate_projected_tensor("q", q)
        if self._key_vectors is None or self._value_vectors is None:
            raise RuntimeError("cannot attend with an empty KV cache")

        batch, heads, query_seq, head_dim = q.shape
        if batch != self._batch:
            raise ValueError("query batch shape must match the KV cache")
        if head_dim != self.codec.dim:
            raise ValueError(
                f"TurboQuant codec dim {self.codec.dim} does not match query head_dim {head_dim}"
            )

        q_rows = q.to_list()
        result = []

        for batch_index in range(batch):
            batch_out = []
            for query_head_index in range(heads):
                kv_head_index = _kv_head_index(heads, self._heads, query_head_index)
                decoded_values = self._decoded_values_for_head(batch_index, kv_head_index)
                head_out = []
                for query_index in range(query_seq):
                    query_row = q_rows[batch_index][query_head_index][query_index]
                    prepared = self.codec.prepare_query(query_row)
                    logits = []
                    for encoded in self._key_vectors[batch_index][kv_head_index]:
                        logits.append(
                            self.codec.estimate_inner_product_prepared(prepared, encoded)
                            * scale
                        )
                    if mask is not None:
                        mask_row = _expanded_mask_row(
                            _broadcast_mask_row(
                                mask,
                                batch_index,
                                query_head_index,
                                query_index,
                            ),
                            len(logits),
                        )
                        logits = [
                            logits[index] + float(mask_row[index])
                            for index in range(len(logits))
                        ]
                    weights = Tensor(logits, shape=(len(logits),)).softmax().to_list()
                    out_row = []
                    for dim_index in range(head_dim):
                        acc = 0.0
                        for value_index, value_row in enumerate(decoded_values):
                            acc += weights[value_index] * float(value_row[dim_index])
                        out_row.append(acc)
                    head_out.append(out_row)
                batch_out.append(head_out)
            result.append(batch_out)

        return Tensor(result)

    def _decoded_values_for_head(
        self, batch_index: int, kv_head_index: int
    ) -> list[list[float]]:
        if self._decoded_value_rows is None:
            self._decoded_value_rows = [
                [[] for _ in range(self._heads)]
                for _ in range(self._batch)
            ]
        cached = self._decoded_value_rows[batch_index][kv_head_index]
        if cached:
            return cached
        rows = [
            self.codec.dequantize(encoded).to_list()
            for encoded in self._value_vectors[batch_index][kv_head_index]
        ]
        self._decoded_value_rows[batch_index][kv_head_index] = rows
        return rows

    def _invalidate_runtime_shadow_rows(self) -> None:
        self._runtime_mse_signs = None
        self._runtime_qjl_signs = None
        self._runtime_key_mse_weight_rows = None
        self._runtime_key_residual_sign_rows = None
        self._runtime_key_residual_scale_rows = None
        self._runtime_value_rows = None

    def _ensure_runtime_shadow_rows(self) -> None:
        if (
            self._runtime_mse_signs is not None
            and self._runtime_qjl_signs is not None
            and self._runtime_key_mse_weight_rows is not None
            and self._runtime_key_residual_sign_rows is not None
            and self._runtime_key_residual_scale_rows is not None
            and self._runtime_value_rows is not None
        ):
            return
        if self._key_vectors is None or self._value_vectors is None:
            raise RuntimeError("cannot build TurboQuant runtime shadow rows from an empty cache")

        batch = self._batch
        heads = self._heads
        seq = len(self)
        dim = self.codec.dim
        key_mse = []
        key_sign = []
        key_scale = []
        value_rows = []

        for batch_index in range(batch):
            for head_index in range(heads):
                decoded_values = self._decoded_values_for_head(batch_index, head_index)
                for seq_index in range(seq):
                    key_encoded = self._key_vectors[batch_index][head_index][seq_index]
                    if key_encoded.mse_weights is not None:
                        key_mse.extend(key_encoded.mse_weights)
                    else:
                        key_mse.extend(
                            key_encoded.norm * self.codec.codebook[int(index)]
                            for index in key_encoded.indices
                        )
                    key_sign.extend(key_encoded.residual_signs)
                    if key_encoded.residual_scale is not None:
                        key_scale.append(key_encoded.residual_scale)
                    else:
                        key_scale.append(
                            math.sqrt(math.pi / 2.0)
                            / float(dim)
                            * key_encoded.residual_norm
                            * key_encoded.norm
                        )
                    value_rows.extend(decoded_values[seq_index])

        self._runtime_key_mse_weight_rows = Tensor(
            key_mse,
            shape=(batch, heads, seq, dim),
        )
        self._runtime_mse_signs = Tensor(
            list(self.codec.mse_rotation.signs),
            shape=(dim,),
        )
        self._runtime_qjl_signs = Tensor(
            list(self.codec.qjl_rotation.signs),
            shape=(dim,),
        )
        self._runtime_key_residual_sign_rows = Tensor(
            key_sign,
            shape=(batch, heads, seq, dim),
        )
        self._runtime_key_residual_scale_rows = Tensor(
            key_scale,
            shape=(batch, heads, seq),
        )
        self._runtime_value_rows = Tensor(
            value_rows,
            shape=(batch, heads, seq, dim),
        )

    def truncate(self, length: int) -> None:
        if length < 0:
            raise ValueError("KV cache length must be non-negative")
        current = len(self)
        if length > current:
            raise ValueError("cannot extend KV cache via truncate")
        if self._key_vectors is None or self._value_vectors is None:
            if length != 0:
                raise ValueError("cannot truncate an empty KV cache to a non-zero length")
            return
        if length == current:
            return
        if length == 0:
            self._key_vectors = None
            self._value_vectors = None
            self._batch = None
            self._heads = None
            self._decoded_value_rows = None
            self._invalidate_runtime_shadow_rows()
            return
        for batch_index in range(self._batch):
            for head_index in range(self._heads):
                del self._key_vectors[batch_index][head_index][length:]
                del self._value_vectors[batch_index][head_index][length:]
        self._decoded_value_rows = None
        self._invalidate_runtime_shadow_rows()

    def keys(self):
        self._ensure_runtime_shadow_rows()
        return _KVCacheKeyView(self)

    def values(self):
        self._ensure_runtime_shadow_rows()
        return _KVCacheValueView(self)


class _KVCacheKeyView:
    def __init__(self, kv_cache) -> None:
        self._kv_cache = kv_cache
        self._kv_role = "key"


class _KVCacheValueView:
    def __init__(self, kv_cache) -> None:
        self._kv_cache = kv_cache
        self._kv_role = "value"
