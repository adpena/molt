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


class PerChannelQuantizedTensor:
    """Per-channel quantized tensor — separate scale/zero_point per row.

    For a (out_features, in_features) weight matrix, each row gets its own
    scale factor. This means small and large values in different output
    channels don't compete for the quantization range.

    Compression: same as per-tensor (8x for INT8, 16x for INT4)
    Accuracy: much better — each row uses its full quantization range.
    """

    def __init__(self, data, scales, zero_points, shape, bits=8):
        self._data = data        # packed int8 or int4 bytes (row-major)
        self.scales = scales           # list of floats, one per row
        self.zero_points = zero_points # list of ints, one per row
        self.shape = shape
        self.bits = bits

    @staticmethod
    def quantize(tensor: Tensor, bits: int = 8) -> 'PerChannelQuantizedTensor':
        """Per-channel quantization — quantize each row independently.

        The tensor must be 2D (out_features, in_features). Each row gets
        its own scale and zero_point computed from that row's min/max range.
        """
        if tensor.ndim != 2:
            raise ValueError(
                f"Per-channel quantization requires a 2D tensor, got {tensor.ndim}D"
            )

        rows, cols = tensor.shape
        data_list = tensor._data_list()

        if bits == 8:
            qmin, qmax = -128, 127
        elif bits == 4:
            qmin, qmax = -8, 7
        else:
            raise ValueError(f"Unsupported bit width: {bits}")

        scales = []
        zero_points = []
        all_quantized = []

        for r in range(rows):
            row_start = r * cols
            row_vals = data_list[row_start:row_start + cols]

            min_val = min(row_vals)
            max_val = max(row_vals)

            scale = (max_val - min_val) / (qmax - qmin) if max_val != min_val else 1.0
            zp = int(round(qmin - min_val / scale)) if scale != 0.0 else 0
            zp = max(qmin, min(qmax, zp))

            scales.append(scale)
            zero_points.append(zp)

            for v in row_vals:
                q = int(round(v / scale + zp)) if scale != 0.0 else 0
                q = max(qmin, min(qmax, q))
                all_quantized.append(q)

        # Pack to bytes
        if bits == 8:
            data = struct.pack(f'{len(all_quantized)}b', *all_quantized)
        elif bits == 4:
            packed = []
            for i in range(0, len(all_quantized), 2):
                lo = (all_quantized[i] + 8) & 0x0F
                hi = ((all_quantized[i + 1] + 8) & 0x0F if i + 1 < len(all_quantized) else 0) << 4
                packed.append(lo | hi)
            data = bytes(packed)

        return PerChannelQuantizedTensor(data, scales, zero_points, tensor.shape, bits)

    def dequantize(self) -> Tensor:
        """Dequantize using per-channel scales."""
        rows, cols = self.shape

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

        # Trim to actual size
        total = rows * cols
        values = values[:total]

        # Dequantize each row with its own scale/zero_point
        float_values = []
        for r in range(rows):
            scale = self.scales[r]
            zp = self.zero_points[r]
            row_start = r * cols
            for c in range(cols):
                v = values[row_start + c]
                float_values.append((v - zp) * scale)

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


class PerChannelQuantizedLinear:
    """Quantized linear layer using per-channel (per-row) quantization.

    Each output channel gets its own scale/zero_point, dramatically improving
    accuracy for INT4 quantization compared to per-tensor quantization.
    """

    def __init__(self, in_features, out_features, bits=8):
        self.in_features = in_features
        self.out_features = out_features
        self.bits = bits
        self.weight_q = None  # PerChannelQuantizedTensor
        self.bias = None  # regular Tensor

    def quantize_from(self, linear_layer):
        """Quantize an existing Linear layer's weights with per-channel scaling."""
        if hasattr(linear_layer, 'weight') and linear_layer.weight is not None:
            self.weight_q = PerChannelQuantizedTensor.quantize(
                linear_layer.weight, self.bits
            )
        if hasattr(linear_layer, 'bias') and linear_layer.bias is not None:
            self.bias = linear_layer.bias

    def __call__(self, x: Tensor) -> Tensor:
        if self.weight_q is None:
            raise RuntimeError(
                "PerChannelQuantizedLinear has no weights — call quantize_from() first"
            )
        # Dequantize weight, compute matmul, add bias
        weight = self.weight_q.dequantize()
        result = x @ weight.T
        if self.bias is not None:
            result = result + self.bias
        return result


def quantize_model_per_channel(model, bits=8):
    """Quantize all Linear layers using per-channel quantization.

    Per-channel quantization computes separate scale/zero_point for each
    output row of the weight matrix, giving much better accuracy than
    per-tensor quantization — especially for INT4.

    Returns a new model with PerChannelQuantizedLinear layers.
    Usage:
        q_model = quantize_model_per_channel(model, bits=4)
        output = q_model(input)  # uses per-channel INT4 weights
    """
    if hasattr(model, 'layers'):
        from .nn import Sequential
        new_layers = []
        for layer in model.layers:
            if hasattr(layer, 'weight') and hasattr(layer, 'in_features'):
                ql = PerChannelQuantizedLinear(
                    layer.in_features, layer.out_features, bits
                )
                ql.quantize_from(layer)
                new_layers.append(ql)
            else:
                new_layers.append(layer)
        return Sequential(*new_layers)
    return model
