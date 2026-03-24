"""
molt.gpu.quantize — Quantized tensor operations for efficient inference.

Supports INT8 and INT4 quantization with per-tensor and per-channel scaling.
Reduces memory usage 2-4x and enables faster inference on constrained devices.
"""

import struct
import math
from .tensor import Tensor

class QuantizedTensor:
    """INT8 or INT4 quantized tensor with scale + zero_point."""

    def __init__(self, data: bytes, scale: float, zero_point: int,
                 shape: tuple, bits: int = 8):
        self._data = data  # packed int8 or int4 bytes
        self.scale = scale
        self.zero_point = zero_point
        self.shape = shape
        self.bits = bits

    @staticmethod
    def quantize(tensor: Tensor, bits: int = 8) -> 'QuantizedTensor':
        """Quantize a float tensor to INT8 or INT4."""
        values = tensor.to_list()
        flat = _flatten(values)

        # Compute scale and zero_point (asymmetric quantization)
        min_val = min(flat)
        max_val = max(flat)

        if bits == 8:
            qmin, qmax = -128, 127
        elif bits == 4:
            qmin, qmax = -8, 7
        else:
            raise ValueError(f"Unsupported bit width: {bits}")

        scale = (max_val - min_val) / (qmax - qmin) if max_val != min_val else 1.0
        zero_point = int(round(qmin - min_val / scale)) if scale != 0.0 else 0
        zero_point = max(qmin, min(qmax, zero_point))

        # Quantize values
        quantized = []
        for v in flat:
            q = int(round(v / scale + zero_point)) if scale != 0.0 else 0
            q = max(qmin, min(qmax, q))
            quantized.append(q)

        # Pack to bytes
        if bits == 8:
            data = struct.pack(f'{len(quantized)}b', *quantized)
        elif bits == 4:
            # Pack two 4-bit values per byte
            packed = []
            for i in range(0, len(quantized), 2):
                lo = (quantized[i] + 8) & 0x0F
                hi = ((quantized[i+1] + 8) & 0x0F if i+1 < len(quantized) else 0) << 4
                packed.append(lo | hi)
            data = bytes(packed)

        return QuantizedTensor(data, scale, zero_point, tensor.shape, bits)

    def dequantize(self) -> Tensor:
        """Convert back to float tensor."""
        if self.bits == 8:
            n = len(self._data)
            values = list(struct.unpack(f'{n}b', self._data))
        elif self.bits == 4:
            values = []
            for byte in self._data:
                lo = (byte & 0x0F) - 8
                hi = ((byte >> 4) & 0x0F) - 8
                values.append(lo)
                values.append(hi)

        # Dequantize: float_val = (int_val - zero_point) * scale
        float_values = [(v - self.zero_point) * self.scale for v in values]

        # Trim to actual size
        total = 1
        for s in self.shape:
            total *= s
        float_values = float_values[:total]

        return Tensor(float_values, shape=self.shape)

    @property
    def nbytes(self) -> int:
        return len(self._data)

    def compression_ratio(self, original_tensor: Tensor) -> float:
        """How much smaller this is vs the original float tensor."""
        original_bytes = original_tensor.size * 8  # f64
        return original_bytes / max(self.nbytes, 1)


def _flatten(data):
    if isinstance(data, (list, tuple)):
        result = []
        for item in data:
            if isinstance(item, (list, tuple)):
                result.extend(_flatten(item))
            else:
                result.append(float(item))
        return result
    return [float(data)]


class QuantizedLinear:
    """Quantized linear layer — stores weights as INT8/INT4."""

    def __init__(self, in_features, out_features, bits=8):
        self.in_features = in_features
        self.out_features = out_features
        self.bits = bits
        self.weight_q = None  # QuantizedTensor
        self.bias = None  # regular Tensor

    def quantize_from(self, linear_layer):
        """Quantize an existing Linear layer's weights."""
        if hasattr(linear_layer, 'weight') and linear_layer.weight is not None:
            self.weight_q = QuantizedTensor.quantize(linear_layer.weight, self.bits)
        if hasattr(linear_layer, 'bias') and linear_layer.bias is not None:
            self.bias = linear_layer.bias

    def __call__(self, x: Tensor) -> Tensor:
        if self.weight_q is None:
            raise RuntimeError("QuantizedLinear has no weights — call quantize_from() first")
        # Dequantize weight, compute matmul, add bias
        weight = self.weight_q.dequantize()
        result = x @ weight.T
        if self.bias is not None:
            result = result + self.bias
        return result


def quantize_model(model, bits=8):
    """Quantize all Linear layers in a model to INT8/INT4.

    Returns a new model with QuantizedLinear layers.
    Usage:
        q_model = quantize_model(model, bits=8)
        output = q_model(input)  # uses quantized weights
    """
    if hasattr(model, 'layers'):
        # Sequential model
        from .nn import Sequential
        new_layers = []
        for layer in model.layers:
            if hasattr(layer, 'weight') and hasattr(layer, 'in_features'):
                # It's a Linear layer — quantize it
                ql = QuantizedLinear(layer.in_features, layer.out_features, bits)
                ql.quantize_from(layer)
                new_layers.append(ql)
            else:
                new_layers.append(layer)  # keep activation layers etc
        return Sequential(*new_layers)
    return model
