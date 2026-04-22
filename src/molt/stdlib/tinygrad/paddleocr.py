"""
PaddleOCR inference implemented using tinygrad Tensor API.

This is PaddleOCR's detector (DBNet) + classifier + recognizer (SVTRv2)
reimplemented as compositions of tinygrad's 26 compute primitives. The same
ONNX weights that PaddlePaddle exports are loaded and executed through
molt's lazy-eval tensor graph, enabling compilation to WebGPU/WASM/native.

Model inventory (PP-OCRv4 mobile):
  - ch_PP-OCRv4_det.onnx         4.7 MB  DBNet text detector
  - ch_ppocr_mobile_v2.0_cls     0.6 MB  Direction classifier
  - ch_PP-OCRv4_rec.onnx        10.8 MB  SVTRv2 text recognizer
  Total: ~16.1 MB — fits in Workers memory / browser cache.

Architecture:
  1. DBNet detector: ResNet backbone + FPN + DB head -> binary text mask
  2. Direction classifier: MobileNetV3 -> 0°/180° orientation
  3. SVTRv2 recognizer: MobileNetV3 backbone + SVTR encoder + CTC head

All ops decompose to: Conv2d, MatMul, Add, Mul, Relu, Sigmoid, Softmax,
BatchNorm, GlobalAvgPool, Reshape, Transpose, Concat, Resize — which are
compositions of the 26 tinygrad primitives (EXP2, LOG2, SIN, SQRT, NEG,
RECIPROCAL, ADD, MUL, MAX, CMPLT, CMPEQ, REDUCE_SUM, REDUCE_MAX, etc.).

Usage:
    from tinygrad.paddleocr import PaddleOCR
    ocr = PaddleOCR()
    ocr.load_detector(det_bytes)
    ocr.load_classifier(cls_bytes)
    ocr.load_recognizer(rec_bytes, charset)
    results = ocr.recognize(image_tensor)
    # -> [{"text": "Invoice #42", "confidence": 0.98, "bbox": [x1,y1,x2,y2]}, ...]
"""

from __future__ import annotations

import struct
from _intrinsics import require_intrinsic as _require_intrinsic
_gpu_device = _require_intrinsic("molt_gpu_prim_device")

from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes


# ---------------------------------------------------------------------------
# ONNX weight parser — minimal protobuf reader for ONNX initializer tensors.
# ONNX uses protobuf: we parse only the fields we need (no protobuf dep).
# ---------------------------------------------------------------------------

class OnnxWeightParser:
    """Extract named weight tensors from an ONNX model file.

    PaddleOCR ONNX models store weights as Constant graph nodes (not
    graph.initializer). Each Constant node has an output name and a
    TensorProto ``value`` attribute containing dims, data_type, and the
    payload in either ``float_data``, ``int64_data``, ``int32_data``, or
    ``raw_data``.

    ONNX data_type mapping:
      1 = float32, 6 = int32, 7 = int64, 11 = double

    Strategy:
      1. Try ``onnx`` library (fast, reliable, handles all edge cases).
      2. Fall back to a minimal protobuf wire-format parser that handles
         both ``graph.initializer`` *and* Constant-node extraction without
         any external dependency.
    """

    # ONNX data_type enum → (struct format char, element byte size)
    _DTYPE_MAP: dict[int, tuple[str, int]] = {
        1: ("f", 4),    # FLOAT
        6: ("i", 4),    # INT32
        7: ("q", 8),    # INT64
        11: ("d", 8),   # DOUBLE
    }

    @staticmethod
    def parse(data: bytes) -> dict[str, tuple[tuple[int, ...], int, list[float] | list[int]]]:
        """Parse ONNX bytes → {name: (shape, data_type, flat_values)}.

        ``data_type`` follows ONNX convention (1=float32, 6=int32, 7=int64).
        ``flat_values`` is a list of float for dtype 1/11 or list of int for
        dtype 6/7.
        """
        try:
            return OnnxWeightParser._parse_with_onnx(data)
        except Exception:
            return OnnxWeightParser._parse_raw_protobuf(data)

    # ------------------------------------------------------------------
    # Strategy 1: onnx library (preferred)
    # ------------------------------------------------------------------
    @staticmethod
    def _parse_with_onnx(data: bytes) -> dict[str, tuple[tuple[int, ...], int, list[float] | list[int]]]:
        import onnx
        from onnx import numpy_helper
        import numpy as np

        model = onnx.load_from_string(data)
        weights: dict[str, tuple[tuple[int, ...], int, list[float] | list[int]]] = {}

        # 1. graph.initializer (standard location)
        for init in model.graph.initializer:
            arr = numpy_helper.to_array(init)
            dtype_code = init.data_type
            shape = tuple(int(d) for d in init.dims)
            if dtype_code in (1, 11):
                values: list[float] | list[int] = arr.astype(np.float32).flatten().tolist()
            elif dtype_code in (6, 7):
                values = arr.flatten().tolist()
            else:
                values = arr.astype(np.float32).flatten().tolist()
                dtype_code = 1
            weights[init.name] = (shape, dtype_code, values)

        # 2. Constant nodes (PaddleOCR stores weights here)
        for node in model.graph.node:
            if node.op_type != "Constant":
                continue
            if not node.output:
                continue
            name = node.output[0]
            for attr in node.attribute:
                if attr.name == "value" and attr.t is not None:
                    t = attr.t
                    shape = tuple(int(d) for d in t.dims) if t.dims else ()
                    dtype_code = t.data_type
                    arr = numpy_helper.to_array(t)
                    if dtype_code in (1, 11):
                        values = arr.astype(np.float32).flatten().tolist()
                    elif dtype_code in (6, 7):
                        values = arr.flatten().tolist()
                    else:
                        values = arr.astype(np.float32).flatten().tolist()
                        dtype_code = 1
                    weights[name] = (shape, dtype_code, values)

        return weights

    # ------------------------------------------------------------------
    # Strategy 2: raw protobuf (no dependencies)
    # ------------------------------------------------------------------
    @staticmethod
    def _parse_raw_protobuf(data: bytes) -> dict[str, tuple[tuple[int, ...], int, list[float] | list[int]]]:
        weights: dict[str, tuple[tuple[int, ...], int, list[float] | list[int]]] = {}

        graph_bytes = OnnxWeightParser._extract_field(data, field_num=7, wire_type=2)
        if graph_bytes is None:
            return weights

        offset = 0
        while offset < len(graph_bytes):
            field_num, wire_type, value, new_offset = OnnxWeightParser._read_field(
                graph_bytes, offset
            )
            if new_offset is None:
                break
            offset = new_offset

            # field 5 = initializer (TensorProto)
            if field_num == 5 and wire_type == 2:
                name, shape, dtype_code, vals = OnnxWeightParser._parse_tensor_proto(value)
                if name and vals is not None:
                    weights[name] = (shape, dtype_code, vals)

            # field 1 = node (NodeProto) — look for Constant ops
            if field_num == 1 and wire_type == 2:
                cname, cshape, cdtype, cvals = OnnxWeightParser._parse_constant_node(value)
                if cname and cvals is not None:
                    weights[cname] = (cshape, cdtype, cvals)

        return weights

    @staticmethod
    def _parse_constant_node(data: bytes) -> tuple[str, tuple[int, ...], int, list[float] | list[int] | None]:
        """Parse a NodeProto looking for Constant op with value attribute."""
        op_type = ""
        output_name = ""
        tensor_data: tuple[str, tuple[int, ...], int, list[float] | list[int] | None] | None = None

        offset = 0
        while offset < len(data):
            fn, wt, value, new_offset = OnnxWeightParser._read_field(data, offset)
            if new_offset is None:
                break
            offset = new_offset
            if fn == 4 and wt == 2:  # op_type
                op_type = value.decode("utf-8", errors="replace")
            elif fn == 2 and wt == 2:  # output
                output_name = value.decode("utf-8", errors="replace")
            elif fn == 5 and wt == 2:  # attribute
                # Parse AttributeProto for name="value", type=TENSOR(4)
                attr_name, attr_tensor = OnnxWeightParser._parse_attribute(value)
                if attr_name == "value" and attr_tensor is not None:
                    tensor_data = attr_tensor

        if op_type != "Constant" or not output_name or tensor_data is None:
            return "", (), 0, None

        _, shape, dtype_code, vals = tensor_data
        return output_name, shape, dtype_code, vals

    @staticmethod
    def _parse_attribute(data: bytes) -> tuple[str, tuple[str, tuple[int, ...], int, list[float] | list[int] | None] | None]:
        """Parse AttributeProto. Returns (attr_name, tensor_data_or_None)."""
        attr_name = ""
        tensor_bytes: bytes | None = None

        offset = 0
        while offset < len(data):
            fn, wt, value, new_offset = OnnxWeightParser._read_field(data, offset)
            if new_offset is None:
                break
            offset = new_offset
            if fn == 1 and wt == 2:  # name
                attr_name = value.decode("utf-8", errors="replace")
            elif fn == 4 and wt == 2:  # t (TensorProto)
                tensor_bytes = value

        if attr_name == "value" and tensor_bytes is not None:
            return attr_name, OnnxWeightParser._parse_tensor_proto(tensor_bytes)
        return attr_name, None

    @staticmethod
    def _parse_tensor_proto(data: bytes) -> tuple[str, tuple[int, ...], int, list[float] | list[int] | None]:
        name = ""
        dims: list[int] = []
        data_type = 0
        raw_data: bytes | None = None
        float_data: list[float] = []
        int64_data: list[int] = []
        int32_data: list[int] = []

        offset = 0
        while offset < len(data):
            field_num, wire_type, value, new_offset = OnnxWeightParser._read_field(
                data, offset
            )
            if new_offset is None:
                break
            offset = new_offset
            if field_num == 1 and wire_type == 2:
                name = value.decode("utf-8", errors="replace")
            elif field_num == 2 and wire_type == 0:
                dims.append(value)
            elif field_num == 2 and wire_type == 2:
                dims.extend(OnnxWeightParser._decode_packed_varints(value))
            elif field_num == 3 and wire_type == 0:
                data_type = value
            elif field_num == 4 and wire_type == 2:
                count = len(value) // 4
                float_data = list(struct.unpack(f"<{count}f", value[:count * 4]))
            elif field_num == 4 and wire_type == 5:
                float_data.append(struct.unpack("<f", value)[0])
            elif field_num == 7 and wire_type == 2:
                # packed repeated int64 (int64_data)
                count = len(value) // 8
                int64_data = list(struct.unpack(f"<{count}q", value[:count * 8]))
            elif field_num == 7 and wire_type == 0:
                int64_data.append(value)
            elif field_num == 8 and wire_type == 2:
                # packed repeated int32 (int32_data, field 8 in some versions)
                count = len(value) // 4
                int32_data = list(struct.unpack(f"<{count}i", value[:count * 4]))
            elif field_num == 13 and wire_type == 2:
                raw_data = value

        shape = tuple(dims)
        dtype_info = OnnxWeightParser._DTYPE_MAP.get(data_type)

        if raw_data is not None and dtype_info is not None:
            fmt_char, elem_size = dtype_info
            count = len(raw_data) // elem_size
            vals: list[float] | list[int] = list(struct.unpack(f"<{count}{fmt_char}", raw_data[:count * elem_size]))
            return name, shape, data_type, vals

        if data_type in (1, 11) and float_data:
            return name, shape, data_type, float_data
        if data_type == 7 and int64_data:
            return name, shape, data_type, int64_data
        if data_type == 6 and int32_data:
            return name, shape, data_type, int32_data
        # Fallback: try float_data for unknown/zero dtype
        if float_data:
            return name, shape, data_type or 1, float_data

        return name, shape, data_type, None

    @staticmethod
    def _extract_field(data: bytes, field_num: int, wire_type: int) -> bytes | None:
        """Extract the first occurrence of a specific field from protobuf data."""
        offset = 0
        while offset < len(data):
            fn, wt, value, new_offset = OnnxWeightParser._read_field(data, offset)
            if new_offset is None:
                break
            if fn == field_num and wt == wire_type:
                return value
            offset = new_offset
        return None

    @staticmethod
    def _read_field(data: bytes, offset: int) -> tuple[int, int, object, int | None]:
        """Read one protobuf field. Returns (field_num, wire_type, value, new_offset)."""
        if offset >= len(data):
            return 0, 0, None, None
        tag, offset = OnnxWeightParser._read_varint(data, offset)
        if offset is None:
            return 0, 0, None, None
        wire_type = tag & 0x7
        field_num = tag >> 3

        if wire_type == 0:  # varint
            value, offset = OnnxWeightParser._read_varint(data, offset)
            return field_num, wire_type, value, offset
        elif wire_type == 1:  # 64-bit
            if offset + 8 > len(data):
                return field_num, wire_type, None, None
            value = data[offset:offset + 8]
            return field_num, wire_type, value, offset + 8
        elif wire_type == 2:  # length-delimited
            length, offset = OnnxWeightParser._read_varint(data, offset)
            if offset is None or offset + length > len(data):
                return field_num, wire_type, None, None
            value = data[offset:offset + length]
            return field_num, wire_type, value, offset + length
        elif wire_type == 5:  # 32-bit
            if offset + 4 > len(data):
                return field_num, wire_type, None, None
            value = data[offset:offset + 4]
            return field_num, wire_type, value, offset + 4
        else:
            return field_num, wire_type, None, None

    @staticmethod
    def _read_varint(data: bytes, offset: int) -> tuple[int, int | None]:
        result = 0
        shift = 0
        while offset < len(data):
            b = data[offset]
            offset += 1
            result |= (b & 0x7F) << shift
            if (b & 0x80) == 0:
                return result, offset
            shift += 7
        return result, None

    @staticmethod
    def _decode_packed_varints(data: bytes) -> list[int]:
        values: list[int] = []
        offset = 0
        while offset < len(data):
            val, offset = OnnxWeightParser._read_varint(data, offset)
            if offset is None:
                break
            values.append(val)
        return values


# ---------------------------------------------------------------------------
# Weight container — maps ONNX node names to tinygrad Tensors.
# ---------------------------------------------------------------------------

class WeightStore:
    """Holds named weight tensors loaded from ONNX."""

    __slots__ = ("_tensors",)

    def __init__(self) -> None:
        self._tensors: dict[str, Tensor] = {}

    def load_onnx(self, data: bytes) -> int:
        """Load weights from ONNX bytes. Returns count of tensors loaded.

        Handles float32 (dtype 1), int32 (dtype 6), and int64 (dtype 7)
        tensors. Integer tensors are stored as-is — they are typically
        shape/index constants used by Reshape, Slice, etc.
        """
        parsed = OnnxWeightParser.parse(data)
        count = 0
        for name, (shape, dtype_code, values) in parsed.items():
            if not shape:
                shape = (len(values),)
            from tinygrad.lazy import LazyOp, LazyBuffer
            if dtype_code in (6, 7):
                # Integer constant (shape params, indices, etc.)
                # Store as int64 for shape operations
                dt = dtypes.int64 if dtype_code == 7 else dtypes.int32
                op = LazyOp("LOAD", (), dtype=dt, shape=shape)
                buf = LazyBuffer(op, dt, shape, data=values)
            else:
                # Float32 (default)
                op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
                buf = LazyBuffer(op, dtypes.float32, shape, data=values)
            self._tensors[name] = Tensor(buf)
            count += 1
        return count

    def get(self, name: str) -> Tensor:
        """Get a weight tensor by name. Raises KeyError if not found."""
        return self._tensors[name]

    def names(self) -> list[str]:
        return list(self._tensors.keys())

    def __len__(self) -> int:
        return len(self._tensors)

    def __contains__(self, name: str) -> bool:
        return name in self._tensors


# ---------------------------------------------------------------------------
# Reusable NN building blocks for PaddleOCR (all tinygrad primitives).
# ---------------------------------------------------------------------------

def _batch_norm(x: Tensor, weight: Tensor, bias: Tensor,
                mean: Tensor, var: Tensor, eps: float = 1e-5) -> Tensor:
    """BatchNormalization in inference mode.

    x: (N, C, H, W)
    weight/bias/mean/var: (C,)
    Output: (N, C, H, W)

    Formula: y = (x - mean) / sqrt(var + eps) * weight + bias
    All ops decompose to tinygrad primitives: SUB, ADD, MUL, SQRT, RECIPROCAL.
    """
    # Reshape params to (1, C, 1, 1) for broadcasting
    c = weight.shape[0]
    w = weight.reshape(1, c, 1, 1)
    b = bias.reshape(1, c, 1, 1)
    m = mean.reshape(1, c, 1, 1)
    v = var.reshape(1, c, 1, 1)

    inv_std = (v + eps).sqrt().reciprocal()
    return (x - m._broadcast_to(x.shape)) * inv_std._broadcast_to(x.shape) * w._broadcast_to(x.shape) + b._broadcast_to(x.shape)


def _conv_bn_relu(x: Tensor, ws: WeightStore, prefix: str,
                  stride: int = 1, padding: int = 0,
                  activation: str = "relu") -> Tensor:
    """Conv2d + BatchNorm + activation — the most common block in PaddleOCR.

    Looks up weights from the WeightStore using PaddlePaddle's naming:
      {prefix}.weight, {prefix}.bias (conv)
      {prefix}.bn.weight, {prefix}.bn.bias, {prefix}.bn.running_mean, {prefix}.bn.running_var
    """
    w = ws.get(f"{prefix}.weight")
    conv_out = Tensor.conv2d(x, w, stride=stride, padding=padding)

    # BatchNorm if weights present
    bn_prefix = f"{prefix}.bn"
    if f"{bn_prefix}.weight" in ws:
        bn_w = ws.get(f"{bn_prefix}.weight")
        bn_b = ws.get(f"{bn_prefix}.bias")
        bn_m = ws.get(f"{bn_prefix}.running_mean")
        bn_v = ws.get(f"{bn_prefix}.running_var")
        conv_out = _batch_norm(conv_out, bn_w, bn_b, bn_m, bn_v)
    elif f"{prefix}.bias" in ws:
        conv_out = conv_out + ws.get(f"{prefix}.bias")

    if activation == "relu":
        return conv_out.relu()
    elif activation == "sigmoid":
        return conv_out.sigmoid()
    elif activation == "hard_sigmoid":
        # hard_sigmoid(x) = clip(x * alpha + beta, 0, 1), alpha=0.2, beta=0.5
        # Equivalent to: clip(x/6 + 0.5, 0, 1) in some implementations
        # PaddleOCR ONNX uses: clip(x * 0.1666... + 0.5, 0, 1)
        return (conv_out * 0.16666667 + 0.5).relu() - (conv_out * 0.16666667 - 0.5).relu()
    elif activation == "none":
        return conv_out
    else:
        return conv_out.relu()


def _global_avg_pool(x: Tensor) -> Tensor:
    """Global average pooling: (N, C, H, W) -> (N, C, 1, 1).

    Decomposed: REDUCE_SUM over spatial dims / (H * W).
    """
    n, c, h, w = x.shape
    spatial = h * w
    # Reshape to (N, C, H*W), sum over last axis, divide, reshape
    flat = x.reshape(n, c, spatial)
    summed = flat.sum(axis=-1)  # (N, C)
    return (summed * (1.0 / spatial)).reshape(n, c, 1, 1)


def _se_block(x: Tensor, ws: WeightStore, prefix: str, reduction: int = 4) -> Tensor:
    """Squeeze-and-Excitation block (used in MobileNetV3 backbone).

    GAP -> FC1 (reduce) -> ReLU -> FC2 (expand) -> HardSigmoid -> scale
    """
    pooled = _global_avg_pool(x)  # (N, C, 1, 1)
    n, c, _, _ = x.shape
    squeezed = pooled.reshape(n, c)

    # FC1: reduce channels
    fc1_w = ws.get(f"{prefix}.fc1.weight")
    fc1_b = ws.get(f"{prefix}.fc1.bias") if f"{prefix}.fc1.bias" in ws else None
    mid = squeezed @ fc1_w.T
    if fc1_b is not None:
        mid = mid + fc1_b
    mid = mid.relu()

    # FC2: expand back
    fc2_w = ws.get(f"{prefix}.fc2.weight")
    fc2_b = ws.get(f"{prefix}.fc2.bias") if f"{prefix}.fc2.bias" in ws else None
    scale = mid @ fc2_w.T
    if fc2_b is not None:
        scale = scale + fc2_b
    # Hard sigmoid: clip(x * 0.1667 + 0.5, 0, 1)
    scale = (scale * 0.16666667 + 0.5).relu() - (scale * 0.16666667 - 0.5).relu()

    scale = scale.reshape(n, c, 1, 1)
    return x * scale._broadcast_to(x.shape)


def _nearest_upsample_2x(x: Tensor) -> Tensor:
    """2x nearest-neighbor upsampling: (N, C, H, W) -> (N, C, 2H, 2W).

    Decomposed to reshape + expand (repeat) operations.
    """
    from tinygrad.lazy import LazyOp, LazyBuffer
    import tinygrad.realize

    flat = tinygrad.realize.realize(x.lazydata)
    n, c, h, w = x.shape
    h2, w2 = h * 2, w * 2
    out_size = n * c * h2 * w2
    result = [0.0] * out_size

    for bn in range(n):
        for ch in range(c):
            for oh in range(h2):
                for ow in range(w2):
                    src_h = oh // 2
                    src_w = ow // 2
                    src_idx = bn * (c * h * w) + ch * (h * w) + src_h * w + src_w
                    dst_idx = bn * (c * h2 * w2) + ch * (h2 * w2) + oh * w2 + ow
                    result[dst_idx] = flat[src_idx]

    out_shape = (n, c, h2, w2)
    op = LazyOp("LOAD", (), dtype=x.dtype, shape=out_shape)
    return Tensor(LazyBuffer(op, x.dtype, out_shape, data=result))


# ---------------------------------------------------------------------------
# DBNet Text Detector
# ---------------------------------------------------------------------------

class PaddleOCRDetector:
    """DBNet text detector — finds text bounding boxes in images.

    PP-OCRv4 detector architecture:
      - Backbone: LCNet (lightweight MobileNet variant) with SE blocks
      - Neck: FPN (Feature Pyramid Network) — 4 scales merged
      - Head: DB (Differentiable Binarization) — probability + threshold maps

    Input:  [1, 3, H, W] normalized image (mean=[0.485, 0.456, 0.406],
                                            std=[0.229, 0.224, 0.225])
    Output: [1, 1, H, W] text probability map (0.0-1.0 per pixel)

    ONNX op breakdown (778 nodes):
      Conv: 62, BatchNorm: 3, Relu: 12, HardSigmoid: 10,
      GlobalAveragePool: 10, Resize: 6, Concat: 1, Sigmoid: 1
    """

    __slots__ = ("weights", "_loaded", "_interpreter")

    def __init__(self) -> None:
        self.weights = WeightStore()
        self._loaded = False
        self._interpreter = None

    def load(self, onnx_bytes: bytes) -> None:
        """Load detector weights from ONNX file bytes.

        Parses both the weight tensors (via WeightStore for backward compat)
        and the full ONNX computation graph (via OnnxInterpreter for forward
        pass execution).
        """
        count = self.weights.load_onnx(onnx_bytes)
        self._loaded = count > 0

        # Load the full ONNX graph for interpreter-based forward pass
        from tinygrad.onnx_interpreter import OnnxInterpreter
        self._interpreter = OnnxInterpreter()
        self._interpreter.load_model(onnx_bytes)

    def load_onnx_weights(self, onnx_bytes: bytes) -> None:
        """Alias for load() — loads detector weights from raw ONNX bytes."""
        self.load(onnx_bytes)

    def forward(self, image: Tensor) -> Tensor:
        """Run text detection via ONNX graph interpreter.

        Args:
            image: [1, 3, H, W] normalized float32 tensor.

        Returns:
            [1, 1, H, W] probability map where values > 0.3 indicate text.

        Executes the full PP-OCRv4 detector ONNX graph (778 nodes, 15 op
        types) through the generic OnnxInterpreter.  Each ONNX op
        decomposes to tinygrad's 26 compute primitives:

          Conv (62 nodes)      -> im2col + matmul (grouped for depthwise)
          BatchNorm (3)        -> (x-mean)/sqrt(var+eps)*w+b
          Relu (12)            -> MAX(x, 0)
          HardSigmoid (10)     -> clip(alpha*x+beta, 0, 1)
          GlobalAvgPool (10)   -> REDUCE_SUM / spatial_size
          Sigmoid (1)          -> 1/(1+exp(-x))
          Add/Mul/Div (251)    -> ADD/MUL/RECIPROCAL
          Reshape (54)         -> view
          Clip (24)            -> relu compositions
          Resize (6)           -> nearest-neighbor upsample
          Concat (1)           -> Tensor.cat
          ConvTranspose (2)    -> transposed conv via scatter-add
        """
        if self._interpreter is None:
            raise RuntimeError("Detector weights not loaded. Call load() first.")

        # The detector ONNX graph input is named "x"
        outputs = self._interpreter.run({"x": image})
        # Return the first (and only) output — sigmoid probability map
        for name, tensor in outputs.items():
            return tensor
        raise RuntimeError("Detector ONNX graph produced no outputs")

    def detect(self, image: Tensor, threshold: float = 0.3,
               min_area: int = 100) -> list[tuple[int, int, int, int]]:
        """Run detection and extract bounding boxes.

        Args:
            image: [1, 3, H, W] normalized input.
            threshold: Probability threshold for text pixels.
            min_area: Minimum bounding box area in pixels.

        Returns:
            List of (x1, y1, x2, y2) bounding boxes in pixel coordinates.
        """
        import tinygrad.realize

        prob_map = self.forward(image)
        flat = tinygrad.realize.realize(prob_map.lazydata)
        _, _, h, w = prob_map.shape

        # Connected-component extraction via flood fill on thresholded map
        visited = [False] * (h * w)
        boxes: list[tuple[int, int, int, int]] = []

        for y in range(h):
            for x_pos in range(w):
                idx = y * w + x_pos
                if visited[idx] or flat[idx] < threshold:
                    continue
                # BFS flood fill
                min_x, min_y, max_x, max_y = x_pos, y, x_pos, y
                queue = [idx]
                visited[idx] = True
                area = 0
                while queue:
                    cur = queue.pop()
                    cy, cx = divmod(cur, w)
                    cy_actual = cur // w
                    cx_actual = cur % w
                    min_x = min(min_x, cx_actual)
                    max_x = max(max_x, cx_actual)
                    min_y = min(min_y, cy_actual)
                    max_y = max(max_y, cy_actual)
                    area += 1
                    # 4-connected neighbors
                    for dy, dx in [(-1, 0), (1, 0), (0, -1), (0, 1)]:
                        ny, nx = cy_actual + dy, cx_actual + dx
                        if 0 <= ny < h and 0 <= nx < w:
                            nidx = ny * w + nx
                            if not visited[nidx] and flat[nidx] >= threshold:
                                visited[nidx] = True
                                queue.append(nidx)

                if area >= min_area:
                    boxes.append((min_x, min_y, max_x, max_y))

        return boxes


# ---------------------------------------------------------------------------
# Direction Classifier
# ---------------------------------------------------------------------------

class PaddleOCRClassifier:
    """Text direction classifier — determines 0° vs 180° orientation.

    MobileNetV3-small backbone (566 ONNX nodes, 0.6 MB weights).
    Input:  [N, 3, 48, 192] text line crops (resized).
    Output: [N, 2] softmax probabilities for [0°, 180°].

    ONNX ops: Conv(53), BatchNorm(35), Relu(15), HardSigmoid(9),
              GlobalAvgPool(10), MaxPool(1), Reshape, Add, Mul, Clip.
    """

    __slots__ = ("weights", "_loaded", "_interpreter")

    def __init__(self) -> None:
        self.weights = WeightStore()
        self._loaded = False
        self._interpreter = None

    def load(self, onnx_bytes: bytes) -> None:
        count = self.weights.load_onnx(onnx_bytes)
        self._loaded = count > 0

        from tinygrad.onnx_interpreter import OnnxInterpreter
        self._interpreter = OnnxInterpreter()
        self._interpreter.load_model(onnx_bytes)

    def forward(self, crops: Tensor) -> Tensor:
        """Classify text direction via ONNX graph interpreter.

        Args:
            crops: [N, 3, 48, 192] batch of text line images.

        Returns:
            [N, 2] softmax output — column 0 = 0°, column 1 = 180°.
        """
        if self._interpreter is None:
            raise RuntimeError("Classifier weights not loaded. Call load() first.")

        outputs = self._interpreter.run({"x": crops})
        for name, tensor in outputs.items():
            return tensor
        raise RuntimeError("Classifier ONNX graph produced no outputs")

    def needs_rotation(self, crops: Tensor, threshold: float = 0.9) -> list[bool]:
        """Returns per-crop boolean: True if crop needs 180° rotation."""
        import tinygrad.realize
        probs = self.forward(crops)
        flat = tinygrad.realize.realize(probs.lazydata)
        n = crops.shape[0]
        return [flat[i * 2 + 1] > threshold for i in range(n)]


# ---------------------------------------------------------------------------
# SVTRv2 Text Recognizer
# ---------------------------------------------------------------------------

class PaddleOCRRecognizer:
    """SVTRv2 text recognizer — reads text from cropped regions.

    PP-OCRv4 recognizer architecture (934 ONNX nodes, 10.8 MB):
      - Backbone: MobileNetV3-like with depthwise separable convs
      - Encoder: SVTR (Scene-Visual Transformer) — self-attention on
        feature sequence from CNN backbone
      - Head: CTC (Connectionist Temporal Classification) decoder

    Input:  [1, 3, 48, W] cropped text line image (height=48, W varies)
    Output: [1, W/4, 6625] character probability distribution

    The 6625 output classes correspond to the PP-OCRv4 character set:
    6623 Chinese/English/symbol characters + blank + space.

    CTC decoding: take argmax per timestep, remove blanks and duplicates.

    ONNX ops: Conv(38), MatMul(13), Reshape(48), Transpose(9), Sigmoid(7),
              Add(141), Mul(100), Div(33), ReduceMean(10), Softmax(1).
    """

    __slots__ = ("weights", "charset", "_loaded", "_interpreter")

    def __init__(self) -> None:
        self.weights = WeightStore()
        self.charset: list[str] = []
        self._loaded = False
        self._interpreter = None

    def load(self, onnx_bytes: bytes, charset_text: str = "") -> None:
        """Load recognizer weights and character set.

        Args:
            onnx_bytes: Raw bytes of the ONNX model file.
            charset_text: Content of the dictionary file (one char per line).
                         PP-OCRv4 uses 6623 characters + blank token.
        """
        count = self.weights.load_onnx(onnx_bytes)
        self._loaded = count > 0

        from tinygrad.onnx_interpreter import OnnxInterpreter
        self._interpreter = OnnxInterpreter()
        self._interpreter.load_model(onnx_bytes)

        # Parse charset: one character per line, blank token (index 0) is implicit
        self.charset = [""]  # index 0 = CTC blank
        for line in charset_text.strip().split("\n"):
            ch = line.strip()
            if ch:
                self.charset.append(ch)
        # Append space if not present
        if " " not in self.charset:
            self.charset.append(" ")

    def load_onnx_weights(self, onnx_bytes: bytes, charset_text: str = "") -> None:
        """Alias for load() — loads recognizer weights from raw ONNX bytes."""
        self.load(onnx_bytes, charset_text)

    def forward(self, crop: Tensor) -> Tensor:
        """Run text recognition via ONNX graph interpreter.

        Args:
            crop: [1, 3, 48, W] preprocessed text line image.

        Returns:
            [1, T, vocab_size] character probability distribution.
            T = W/4 (due to stride-4 in backbone).

        Executes the full PP-OCRv4 recognizer ONNX graph (934 nodes, 26 op
        types) through the generic OnnxInterpreter. The graph includes:
          Conv (38)         -> im2col + matmul (grouped for depthwise)
          MatMul (13)       -> dot product (SVTR attention)
          Add/Mul/Div (274) -> elementwise arithmetic
          ReduceMean (10)   -> LayerNorm mean computation
          Reshape (48)      -> view
          Transpose (9)     -> permute (attention head reshaping)
          Sigmoid (7)       -> gate activations
          Softmax (3)       -> attention + CTC output
          BatchNorm (6)     -> backbone normalization
        """
        if self._interpreter is None:
            raise RuntimeError("Recognizer weights not loaded. Call load() first.")

        outputs = self._interpreter.run({"x": crop})
        for name, tensor in outputs.items():
            return tensor
        raise RuntimeError("Recognizer ONNX graph produced no outputs")

    def decode_ctc(self, logits: Tensor) -> tuple[str, float]:
        """CTC greedy decode: argmax -> remove blanks and duplicates.

        Args:
            logits: [1, T, vocab_size] raw logits (pre-softmax or post-softmax).

        Returns:
            (decoded_text, average_confidence)
        """
        import tinygrad.realize

        # Apply softmax to get probabilities
        probs = logits.softmax(axis=-1)
        flat = tinygrad.realize.realize(probs.lazydata)

        _, t, vocab = logits.shape
        chars: list[str] = []
        confidences: list[float] = []
        prev_idx = -1

        for step in range(t):
            # Find argmax for this timestep
            best_idx = 0
            best_prob = flat[step * vocab]
            for v in range(1, vocab):
                p = flat[step * vocab + v]
                if p > best_prob:
                    best_prob = p
                    best_idx = v

            # CTC: skip blank (idx 0) and consecutive duplicates
            if best_idx != 0 and best_idx != prev_idx:
                if best_idx < len(self.charset):
                    chars.append(self.charset[best_idx])
                    confidences.append(best_prob)
            prev_idx = best_idx

        text = "".join(chars)
        avg_conf = sum(confidences) / len(confidences) if confidences else 0.0
        return text, avg_conf

    def recognize(self, crop: Tensor) -> tuple[str, float]:
        """Full recognize pipeline: forward + CTC decode.

        Args:
            crop: [1, 3, 48, W] preprocessed text line image.

        Returns:
            (text, confidence)
        """
        logits = self.forward(crop)
        return self.decode_ctc(logits)


# ---------------------------------------------------------------------------
# Full PaddleOCR Pipeline
# ---------------------------------------------------------------------------

class PaddleOCR:
    """Full PaddleOCR pipeline: detect text regions -> classify direction -> recognize text.

    Model sizes (PP-OCRv4 mobile):
      Detector:    4.7 MB  (DBNet with LCNet backbone)
      Classifier:  0.6 MB  (MobileNetV3-small)
      Recognizer: 10.8 MB  (SVTRv2 with CTC head)
      Total:      16.1 MB

    All computation decomposes to tinygrad's 26 primitives, enabling
    compilation through molt to WebGPU, WASM, native, or LLVM backends.
    """

    __slots__ = ("detector", "classifier", "recognizer")

    def __init__(self) -> None:
        self.detector = PaddleOCRDetector()
        self.classifier = PaddleOCRClassifier()
        self.recognizer = PaddleOCRRecognizer()

    def load_detector(self, onnx_bytes: bytes) -> None:
        """Load text detector ONNX weights."""
        self.detector.load(onnx_bytes)

    def load_classifier(self, onnx_bytes: bytes) -> None:
        """Load direction classifier ONNX weights."""
        self.classifier.load(onnx_bytes)

    def load_recognizer(self, onnx_bytes: bytes, charset_text: str) -> None:
        """Load text recognizer ONNX weights and character dictionary."""
        self.recognizer.load(onnx_bytes, charset_text)

    def preprocess(self, image: Tensor) -> Tensor:
        """Normalize image tensor for PaddleOCR.

        PaddleOCR expects: (pixel / 255.0 - mean) / std
          mean = [0.485, 0.456, 0.406]
          std  = [0.229, 0.224, 0.225]

        Input:  [1, 3, H, W] uint8 or float [0, 255]
        Output: [1, 3, H, W] normalized float32
        """
        # Scale to [0, 1]
        x = image * (1.0 / 255.0)
        # Per-channel normalization (channel dim = 1)
        # Subtract mean, divide by std
        # mean/std are applied per-channel: reshape to (1, 3, 1, 1) for broadcast
        from tinygrad.lazy import LazyOp, LazyBuffer
        mean_data = [0.485, 0.456, 0.406]
        std_data = [0.229, 0.224, 0.225]
        mean_t = Tensor(LazyBuffer(
            LazyOp("LOAD", (), dtype=dtypes.float32, shape=(1, 3, 1, 1)),
            dtypes.float32, (1, 3, 1, 1), data=mean_data
        ))
        std_t = Tensor(LazyBuffer(
            LazyOp("LOAD", (), dtype=dtypes.float32, shape=(1, 3, 1, 1)),
            dtypes.float32, (1, 3, 1, 1), data=std_data
        ))
        return (x - mean_t._broadcast_to(x.shape)) * std_t._broadcast_to(x.shape).reciprocal()

    def _crop_region(self, image: Tensor, box: tuple[int, int, int, int]) -> Tensor:
        """Crop a text region from the image and resize to recognizer input.

        Args:
            image: [1, 3, H, W] source image.
            box: (x1, y1, x2, y2) bounding box.

        Returns:
            [1, 3, 48, W'] cropped and resized tensor (height=48).
        """
        x1, y1, x2, y2 = box
        # Shrink to bounding box region
        _, _, h, w = image.shape
        x1 = max(0, x1)
        y1 = max(0, y1)
        x2 = min(w, x2 + 1)
        y2 = min(h, y2 + 1)

        crop = image.shrink([(0, 1), (0, 3), (y1, y2), (x1, x2)])

        # Resize height to 48, scale width proportionally
        crop_h = y2 - y1
        crop_w = x2 - x1
        if crop_h <= 0 or crop_w <= 0:
            return Tensor.zeros(1, 3, 48, 48)

        new_w = max(1, int(48.0 * crop_w / crop_h))
        from tinygrad.onnx_interpreter import _nearest_resize
        return _nearest_resize(crop, (1, 3, 48, new_w))

    def _resize_for_classifier(self, crop: Tensor) -> Tensor:
        """Resize a text crop to the direction classifier's fixed input."""
        from tinygrad.onnx_interpreter import _nearest_resize
        return _nearest_resize(crop, (1, 3, 48, 192))

    def _rotate_crop_180(self, crop: Tensor) -> Tensor:
        """Rotate a [1, C, H, W] crop by 180 degrees."""
        import tinygrad.realize
        from tinygrad.lazy import LazyOp, LazyBuffer

        flat = tinygrad.realize.realize(crop.lazydata)
        n, c, h, w = crop.shape
        out = [0.0] * len(flat)
        for bn in range(n):
            for ch in range(c):
                for y in range(h):
                    for x in range(w):
                        src_idx = bn * c * h * w + ch * h * w + y * w + x
                        dst_y = h - 1 - y
                        dst_x = w - 1 - x
                        dst_idx = bn * c * h * w + ch * h * w + dst_y * w + dst_x
                        out[dst_idx] = flat[src_idx]
        op = LazyOp("LOAD", (), dtype=crop.dtype, shape=crop.shape)
        return Tensor(LazyBuffer(op, crop.dtype, crop.shape, data=out))

    def recognize(self, image: Tensor) -> list[dict]:
        """Full OCR pipeline: detect -> classify -> recognize.

        Args:
            image: [1, 3, H, W] raw image tensor (pixel values 0-255).

        Returns:
            List of dicts with keys:
              "text": recognized text string
              "confidence": float 0-1
              "bbox": [x1, y1, x2, y2] pixel coordinates
        """
        # 1. Preprocess
        normalized = self.preprocess(image)

        # 2. Detect text regions
        boxes = self.detector.detect(normalized)

        if not boxes:
            return []

        # 3. Crop each region
        crops = [self._crop_region(normalized, box) for box in boxes]

        # 4. Classify direction and rotate when the classifier is loaded.
        if self.classifier._interpreter is not None:
            oriented_crops = []
            for crop in crops:
                cls_crop = self._resize_for_classifier(crop)
                rotate = self.classifier.needs_rotation(cls_crop)[0]
                oriented_crops.append(self._rotate_crop_180(crop) if rotate else crop)
            crops = oriented_crops

        # 5. Recognize text in each crop
        results: list[dict] = []
        for crop, box in zip(crops, boxes):
            text, conf = self.recognizer.recognize(crop)
            results.append({
                "text": text,
                "confidence": conf,
                "bbox": list(box),
            })

        # 6. Sort by vertical position (top-to-bottom reading order)
        results.sort(key=lambda r: (r["bbox"][1], r["bbox"][0]))

        return results


# ---------------------------------------------------------------------------
# Convenience: load all models from file paths or bytes
# ---------------------------------------------------------------------------

def load_paddleocr(detector_path: str = None, classifier_path: str = None,
                   recognizer_path: str = None, charset_path: str = None,
                   detector_bytes: bytes = None, classifier_bytes: bytes = None,
                   recognizer_bytes: bytes = None, charset_text: str = None) -> PaddleOCR:
    """Create a fully loaded PaddleOCR instance.

    Accepts either file paths or raw bytes for each component.

    Args:
        detector_path: Path to ch_PP-OCRv4_det.onnx
        classifier_path: Path to ch_ppocr_mobile_v2.0_cls_infer.onnx
        recognizer_path: Path to ch_PP-OCRv4_rec.onnx
        charset_path: Path to ppocrv5_dict.txt
        detector_bytes: Raw bytes of detector ONNX
        classifier_bytes: Raw bytes of classifier ONNX
        recognizer_bytes: Raw bytes of recognizer ONNX
        charset_text: Character set as string (one char per line)

    Returns:
        Configured PaddleOCR instance ready for inference.
    """
    ocr = PaddleOCR()

    if detector_path:
        with open(detector_path, "rb") as f:
            detector_bytes = f.read()
    if detector_bytes:
        ocr.load_detector(detector_bytes)

    if classifier_path:
        with open(classifier_path, "rb") as f:
            classifier_bytes = f.read()
    if classifier_bytes:
        ocr.load_classifier(classifier_bytes)

    if charset_path and not charset_text:
        with open(charset_path, "r") as f:
            charset_text = f.read()

    if recognizer_path:
        with open(recognizer_path, "rb") as f:
            recognizer_bytes = f.read()
    if recognizer_bytes:
        ocr.load_recognizer(recognizer_bytes, charset_text or "")

    return ocr
