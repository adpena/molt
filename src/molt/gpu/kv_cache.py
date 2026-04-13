"""KV-cache backends for projected attention tensors."""

from __future__ import annotations

from .tensor import Tensor, tensor_permute_dims, tensor_softmax_last_axis
from .turboquant import TurboQuantCodec


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


class DenseKVCache:
    """Reference dense KV cache for projected multi-head attention tensors."""

    def __init__(self) -> None:
        self._key_chunks = []
        self._value_chunks = []
        self._batch = None
        self._heads = None
        self._head_dim = None
        self._length = 0
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
        self.key_tensor = None
        self.value_tensor = None

    def _materialize(self) -> tuple[Tensor, Tensor]:
        if self.key_tensor is not None and self.value_tensor is not None:
            return self.key_tensor, self.value_tensor
        if not self._key_chunks or not self._value_chunks:
            raise RuntimeError("cannot materialize an empty KV cache")
        key = self._key_chunks[0]
        value = self._value_chunks[0]
        for chunk in self._key_chunks[1:]:
            key = key.cat(chunk, dim=2)
        for chunk in self._value_chunks[1:]:
            value = value.cat(chunk, dim=2)
        self.key_tensor = key
        self.value_tensor = value
        return key, value

    def attention(self, q: Tensor, *, scale: float, mask: Tensor | None = None) -> Tensor:
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
        else:
            repeats = heads // self._heads if self._heads else 0
            if heads < self._heads or heads % self._heads != 0:
                raise ValueError(
                    f"query head count {heads} is incompatible with kv head count {self._heads}"
                )
            expanded_key = key_tensor.repeat_axis(1, repeats)
            expanded_value = value_tensor.repeat_axis(1, repeats)

        scores = (q @ tensor_permute_dims(expanded_key, (0, 1, 3, 2))) * scale
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
            self.key_tensor = None
            self.value_tensor = None
            return
        key_tensor, value_tensor = self._materialize()
        self.key_tensor = key_tensor[:, :, :length, :]
        self.value_tensor = value_tensor[:, :, :length, :]
        self._key_chunks = [self.key_tensor]
        self._value_chunks = [self.value_tensor]
        self._length = length

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

    def attention(self, q: Tensor, *, scale: float, mask: Tensor | None = None) -> Tensor:
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
                decoded_values = [
                    self.codec.dequantize(encoded).to_list()
                    for encoded in self._value_vectors[batch_index][kv_head_index]
                ]
                head_out = []
                for query_index in range(query_seq):
                    query_row = q_rows[batch_index][query_head_index][query_index]
                    logits = []
                    for encoded in self._key_vectors[batch_index][kv_head_index]:
                        logits.append(
                            self.codec.estimate_inner_product(query_row, encoded) * scale
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
            return
        for batch_index in range(self._batch):
            for head_index in range(self._heads):
                del self._key_vectors[batch_index][head_index][length:]
                del self._value_vectors[batch_index][head_index][length:]

    def keys(self):
        return _KVCacheKeyView(self)

    def values(self):
        return _KVCacheValueView(self)


class _KVCacheKeyView:
    def __init__(self, kv_cache) -> None:
        self._kv_cache = kv_cache
        self._kv_role = "key"


class _KVCacheValueView:
    def __init__(self, kv_cache) -> None:
        self._kv_cache = kv_cache
        self._kv_role = "value"
