"""TurboQuant codec and row-wise KV-cache helpers."""

from __future__ import annotations

import math

from ..tensor import Tensor
from .codebooks import build_codebook
from .contracts import TurboQuantConfig, TurboQuantMSEVector, TurboQuantProdVector
from .rotation import HadamardRotationPlan


class TurboQuantPreparedQuery:
    """Prepared query transforms for repeated TurboQuant score evaluation."""

    def __init__(self, *, query_values, rotated_query, query_sketch) -> None:
        self.query_values = list(query_values)
        self.rotated_query = list(rotated_query)
        self.query_sketch = list(query_sketch)


def _coerce_vector(values, *, dim: int) -> list[float]:
    if isinstance(values, Tensor):
        data = values.to_list()
    else:
        data = values
    if not isinstance(data, list):
        data = list(data)
    if len(data) != dim:
        raise ValueError(f"expected vector with dimension {dim}, got {len(data)}")
    out = []
    for value in data:
        if isinstance(value, list):
            raise ValueError("TurboQuant expects flat 1D vectors")
        out.append(float(value))
    return out


def _coerce_matrix(values, *, dim: int) -> list[list[float]]:
    if isinstance(values, Tensor):
        data = values.to_list()
    else:
        data = values
    rows = []
    for row in data:
        rows.append(_coerce_vector(row, dim=dim))
    return rows


def _vector_norm(values) -> float:
    return math.sqrt(sum(value * value for value in values))


def _nearest_codebook_index(value: float, codebook) -> int:
    best_index = 0
    best_error = abs(value - codebook[0])
    for index in range(1, len(codebook)):
        error = abs(value - codebook[index])
        if error < best_error:
            best_error = error
            best_index = index
    return best_index


def _dot(lhs, rhs) -> float:
    return sum(float(lhs[index]) * float(rhs[index]) for index in range(len(lhs)))


class TurboQuantCodec:
    """Structured-rotation TurboQuant reference codec."""

    def __init__(
        self,
        *,
        dim: int,
        bits: int,
        seed: int = 0,
        qjl_seed: int | None = None,
        rotation: str = "hadamard",
    ) -> None:
        self.config = TurboQuantConfig(
            dim=dim,
            bits=bits,
            seed=seed,
            qjl_seed=qjl_seed,
            rotation=rotation,
        )
        self.mse_rotation = HadamardRotationPlan(dim, self.config.seed)
        self.qjl_rotation = HadamardRotationPlan(dim, self.config.qjl_seed)
        self.codebook = build_codebook(dim, self.config.stage_bits)

    @property
    def dim(self) -> int:
        return self.config.dim

    def _encode_direction(self, unit_vector) -> list[int]:
        rotated = self.mse_rotation.apply(unit_vector)
        return [_nearest_codebook_index(value, self.codebook) for value in rotated]

    def _decode_unit_direction(self, indices) -> list[float]:
        rotated = [self.codebook[int(index)] for index in indices]
        return self.mse_rotation.invert(rotated)

    def quantize_mse(self, vector) -> TurboQuantMSEVector:
        values = _coerce_vector(vector, dim=self.dim)
        norm = _vector_norm(values)
        if norm == 0.0:
            return TurboQuantMSEVector(
                [0] * self.dim, norm=0.0, mse_weights=[0.0] * self.dim
            )
        unit = [value / norm for value in values]
        indices = self._encode_direction(unit)
        mse_weights = [norm * self.codebook[int(index)] for index in indices]
        return TurboQuantMSEVector(indices, norm=norm, mse_weights=mse_weights)

    def quantize_prod(self, vector) -> TurboQuantProdVector:
        values = _coerce_vector(vector, dim=self.dim)
        norm = _vector_norm(values)
        if norm == 0.0:
            return TurboQuantProdVector(
                [0] * self.dim,
                norm=0.0,
                residual_signs=[1.0] * self.dim,
                residual_norm=0.0,
                mse_weights=[0.0] * self.dim,
                residual_scale=0.0,
            )
        unit = [value / norm for value in values]
        indices = self._encode_direction(unit)
        mse_unit = self._decode_unit_direction(indices)
        residual = [unit[index] - mse_unit[index] for index in range(self.dim)]
        residual_norm = _vector_norm(residual)
        residual_sketch = self.qjl_rotation.apply(residual)
        residual_signs = [1.0 if value >= 0.0 else -1.0 for value in residual_sketch]
        mse_weights = [norm * self.codebook[int(index)] for index in indices]
        residual_scale = (
            math.sqrt(math.pi / 2.0) / float(self.dim) * residual_norm * norm
        )
        return TurboQuantProdVector(
            indices,
            norm=norm,
            residual_signs=residual_signs,
            residual_norm=residual_norm,
            mse_weights=mse_weights,
            residual_scale=residual_scale,
        )

    def dequantize(self, encoded) -> Tensor:
        unit = self._decode_unit_direction(encoded.indices)
        if isinstance(encoded, TurboQuantProdVector):
            scale = math.sqrt(math.pi / 2.0) * (encoded.residual_norm / float(self.dim))
            residual = self.qjl_rotation.invert(
                [scale * float(sign) for sign in encoded.residual_signs]
            )
            unit = [unit[index] + residual[index] for index in range(self.dim)]
        return Tensor([encoded.norm * value for value in unit], shape=(self.dim,))

    def prepare_query(self, query) -> TurboQuantPreparedQuery:
        query_values = _coerce_vector(query, dim=self.dim)
        return TurboQuantPreparedQuery(
            query_values=query_values,
            rotated_query=self.mse_rotation.apply(query_values),
            query_sketch=self.qjl_rotation.apply(query_values),
        )

    def estimate_mse_inner_product_prepared(
        self,
        prepared: TurboQuantPreparedQuery,
        encoded: TurboQuantMSEVector,
    ) -> float:
        estimate = 0.0
        if encoded.mse_weights is not None:
            for index, weight in enumerate(encoded.mse_weights):
                estimate += prepared.rotated_query[index] * weight
            return estimate
        for index, code_index in enumerate(encoded.indices):
            estimate += prepared.rotated_query[index] * self.codebook[int(code_index)]
        return encoded.norm * estimate

    def estimate_mse_inner_product(self, query, encoded: TurboQuantMSEVector) -> float:
        prepared = self.prepare_query(query)
        return self.estimate_mse_inner_product_prepared(prepared, encoded)

    def estimate_inner_product_prepared(
        self,
        prepared: TurboQuantPreparedQuery,
        encoded,
    ) -> float:
        mse_estimate = self.estimate_mse_inner_product_prepared(prepared, encoded)
        if not isinstance(encoded, TurboQuantProdVector):
            return mse_estimate
        residual_term = _dot(prepared.query_sketch, encoded.residual_signs)
        if encoded.residual_scale is not None:
            residual_term *= encoded.residual_scale
        else:
            residual_term *= math.sqrt(math.pi / 2.0) / float(self.dim)
            residual_term *= encoded.residual_norm * encoded.norm
        return mse_estimate + residual_term

    def estimate_inner_product(self, query, encoded) -> float:
        prepared = self.prepare_query(query)
        return self.estimate_inner_product_prepared(prepared, encoded)


class TurboQuantKVCache:
    """Row-wise TurboQuant wrapper for key/value caches."""

    def __init__(self, codec: TurboQuantCodec, key_vectors, value_vectors) -> None:
        self.codec = codec
        self.key_vectors = list(key_vectors)
        self.value_vectors = list(value_vectors)
        self._decoded_value_rows = None

    @classmethod
    def from_tensors(cls, codec: TurboQuantCodec, keys, values) -> "TurboQuantKVCache":
        key_rows = _coerce_matrix(keys, dim=codec.dim)
        value_rows = _coerce_matrix(values, dim=codec.dim)
        if len(key_rows) != len(value_rows):
            raise ValueError(
                "TurboQuant KV cache requires matching key/value row counts"
            )
        key_vectors = [codec.quantize_prod(row) for row in key_rows]
        value_vectors = [codec.quantize_prod(row) for row in value_rows]
        return cls(codec, key_vectors, value_vectors)

    def attention_logits(self, query) -> Tensor:
        prepared = self.codec.prepare_query(query)
        logits = [
            self.codec.estimate_inner_product_prepared(prepared, encoded)
            for encoded in self.key_vectors
        ]
        return Tensor(logits, shape=(len(logits),))

    def _decoded_values(self) -> list[list[float]]:
        if self._decoded_value_rows is None:
            self._decoded_value_rows = [
                self.codec.dequantize(encoded).to_list()
                for encoded in self.value_vectors
            ]
        return self._decoded_value_rows

    def attention_output(self, query) -> Tensor:
        logits = self.attention_logits(query)
        weights = logits.softmax().to_list()
        decoded_values = self._decoded_values()
        out = []
        for dim_index in range(self.codec.dim):
            acc = 0.0
            for row_index, row in enumerate(decoded_values):
                acc += weights[row_index] * row[dim_index]
            out.append(acc)
        return Tensor(out, shape=(self.codec.dim,))
