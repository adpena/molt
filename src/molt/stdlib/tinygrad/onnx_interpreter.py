"""
Generic ONNX graph interpreter using tinygrad Tensor operations.

Walks an ONNX computation graph node-by-node and dispatches each op to
tinygrad primitive compositions.  This is more powerful than hand-coding
any single model architecture because it works for ANY ONNX model —
PaddleOCR v3/v4/v5, ResNet, MobileNet, BERT, etc.

Supported op set (29 ops — covers PaddleOCR detector + recognizer):
  Arithmetic:  Add, Sub, Mul, Div, Pow, Sqrt, Sigmoid, Relu, Clip,
               HardSigmoid, HardSwish, Softmax
  Reduction:   ReduceMean, GlobalAveragePool, AveragePool
  Convolution: Conv (with groups), ConvTranspose
  Linear:      MatMul
  Shape:       Reshape, Transpose, Squeeze, Unsqueeze, Concat, Slice,
               Shape, Cast, Identity
  Spatial:     Resize (nearest)
  Constant:    Constant (weight loading)

All ops decompose to tinygrad's 26 compute primitives.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
_gpu_device = _require_intrinsic("molt_gpu_prim_device")

from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes


# ---------------------------------------------------------------------------
# ONNX attribute helpers
# ---------------------------------------------------------------------------

def _get_attr_int(attrs: dict, name: str, default: int = 0) -> int:
    """Get an integer attribute, handling single-element lists."""
    v = attrs.get(name, default)
    if isinstance(v, (list, tuple)):
        return v[0] if v else default
    return int(v)


def _get_attr_ints(attrs: dict, name: str, default: list[int] | None = None) -> list[int]:
    """Get an integer-list attribute."""
    v = attrs.get(name, default)
    if v is None:
        return [] if default is None else default
    if isinstance(v, (list, tuple)):
        return [int(x) for x in v]
    return [int(v)]


def _get_attr_float(attrs: dict, name: str, default: float = 0.0) -> float:
    v = attrs.get(name, default)
    if isinstance(v, (list, tuple)):
        return float(v[0]) if v else default
    return float(v)


def _get_attr_string(attrs: dict, name: str, default: str = "") -> str:
    v = attrs.get(name, default)
    if isinstance(v, bytes):
        return v.decode("utf-8")
    return str(v) if v else default


# ---------------------------------------------------------------------------
# ONNX Graph Interpreter
# ---------------------------------------------------------------------------

class OnnxInterpreter:
    """Execute an ONNX graph using tinygrad Tensor operations.

    Usage:
        interp = OnnxInterpreter()
        interp.load_model(onnx_bytes)
        outputs = interp.run({"x": input_tensor})
    """

    def __init__(self) -> None:
        self._values: dict[str, Tensor] = {}
        self._graph_nodes: list[dict] = []
        self._input_names: list[str] = []
        self._output_names: list[str] = []
        self._op_profile: dict[str, list[float]] = {}  # op_type -> [elapsed_ms, ...]

    def load_model(self, onnx_bytes: bytes) -> None:
        """Parse ONNX model and prepare for execution.

        Extracts the computation graph (nodes, constants, I/O names).
        Uses the onnx library for reliability. The raw protobuf weight parser
        is intentionally not used as a graph-execution fallback because ONNX
        graph decoding requires node, edge, attribute, and I/O metadata.
        """
        try:
            self._load_with_onnx(onnx_bytes)
        except ImportError as exc:
            raise RuntimeError(
                "ONNX graph execution requires the 'onnx' package. "
                "The built-in raw parser is weight-only and cannot execute graphs."
            ) from exc
        self.optimize_graph()

    def run(self, inputs: dict[str, Tensor], profile: bool = False) -> dict[str, Tensor]:
        """Execute the ONNX graph with the given input tensors.

        Args:
            inputs: Map of input name -> Tensor (e.g. {"x": image_tensor}).
            profile: If True, collect per-op timing in self._op_profile.

        Returns:
            Map of output name -> Tensor.
        """
        import time as _time

        if profile:
            self._op_profile.clear()

        values = dict(self._values)  # start with constants
        values.update(inputs)

        for node in self._graph_nodes:
            op_type = node["op_type"]
            op_func = _OP_DISPATCH.get(op_type)
            if op_func is None:
                raise ValueError(f"Unsupported ONNX op: {op_type}")

            # Gather inputs — some may be empty strings (optional)
            input_tensors: list[Tensor | None] = []
            for name in node["inputs"]:
                if name and name in values:
                    input_tensors.append(values[name])
                else:
                    input_tensors.append(None)

            if profile:
                t0 = _time.perf_counter()

            outputs = op_func(input_tensors, node["attrs"])

            if profile:
                elapsed_ms = (_time.perf_counter() - t0) * 1000.0
                if op_type not in self._op_profile:
                    self._op_profile[op_type] = []
                self._op_profile[op_type].append(elapsed_ms)

            for name, tensor in zip(node["outputs"], outputs):
                if name:
                    values[name] = tensor

        return {name: values[name] for name in self._output_names if name in values}

    def profile_summary(self) -> str:
        """Return a formatted summary of per-op profiling data.

        Call run(inputs, profile=True) first to populate timing data.
        """
        if not self._op_profile:
            return "No profiling data. Call run(inputs, profile=True) first."

        lines = ["Op Type          Count   Total(ms)  Mean(ms)  % Total"]
        lines.append("-" * 60)

        total_ms = sum(sum(times) for times in self._op_profile.values())
        entries = []
        for op_type, times in self._op_profile.items():
            total_op = sum(times)
            entries.append((op_type, len(times), total_op, total_op / len(times)))

        entries.sort(key=lambda e: -e[2])  # sort by total time descending

        for op_type, count, total_op, mean_op in entries:
            pct = (total_op / total_ms * 100.0) if total_ms > 0 else 0.0
            lines.append(f"{op_type:<17s} {count:>4d}   {total_op:>9.2f}  {mean_op:>8.3f}  {pct:>6.1f}%")

        lines.append("-" * 60)
        lines.append(f"{'TOTAL':<17s} {sum(len(t) for t in self._op_profile.values()):>4d}   {total_ms:>9.2f}")
        return "\n".join(lines)

    # ------------------------------------------------------------------
    # Graph optimizations
    # ------------------------------------------------------------------

    def optimize_graph(self) -> None:
        """Apply graph-level optimizations before execution.

        Currently implemented:
          - Fold BatchNormalization into preceding Conv (eliminates BN entirely)
          - Eliminate Identity nodes
        """
        self._fold_batchnorm()
        self._eliminate_identity()

    def _fold_batchnorm(self) -> None:
        """Fold Conv -> BatchNormalization pairs into a single Conv.

        For each BatchNormalization whose sole input comes from a Conv output,
        fuse the BN parameters (gamma, beta, mean, var) into the Conv weight
        and bias, then remove the BN node.

        new_weight = weight * (gamma / sqrt(var + eps))   [per output channel]
        new_bias   = (old_bias - mean) * (gamma / sqrt(var + eps)) + beta
        """
        import tinygrad.realize

        # Build output->node index
        output_to_idx: dict[str, int] = {}
        for i, node in enumerate(self._graph_nodes):
            for o in node["outputs"]:
                if o:
                    output_to_idx[o] = i

        # Track which BN nodes to remove and which Conv nodes to update
        remove_indices: set[int] = set()

        for bn_idx, node in enumerate(self._graph_nodes):
            if node["op_type"] != "BatchNormalization":
                continue

            bn_input = node["inputs"][0]
            if bn_input not in output_to_idx:
                continue

            conv_idx = output_to_idx[bn_input]
            conv_node = self._graph_nodes[conv_idx]
            if conv_node["op_type"] != "Conv":
                continue

            # Get BN parameters: scale(gamma), bias(beta), mean, var
            bn_scale_name = node["inputs"][1] if len(node["inputs"]) > 1 else None
            bn_bias_name = node["inputs"][2] if len(node["inputs"]) > 2 else None
            bn_mean_name = node["inputs"][3] if len(node["inputs"]) > 3 else None
            bn_var_name = node["inputs"][4] if len(node["inputs"]) > 4 else None

            if not all(n and n in self._values for n in [bn_scale_name, bn_bias_name, bn_mean_name, bn_var_name]):
                continue

            bn_gamma = self._values[bn_scale_name]
            bn_beta = self._values[bn_bias_name]
            bn_mean = self._values[bn_mean_name]
            bn_var = self._values[bn_var_name]
            eps = _get_attr_float(node["attrs"], "epsilon", 1e-5)

            # Get Conv weight and optional bias
            conv_weight_name = conv_node["inputs"][1] if len(conv_node["inputs"]) > 1 else None
            conv_bias_name = conv_node["inputs"][2] if len(conv_node["inputs"]) > 2 else None

            if not conv_weight_name or conv_weight_name not in self._values:
                continue

            conv_weight = self._values[conv_weight_name]
            conv_bias = self._values[conv_bias_name] if conv_bias_name and conv_bias_name in self._values else None

            # Realize all parameters to flat lists
            gamma_data = list(tinygrad.realize.realize(bn_gamma.lazydata))
            beta_data = list(tinygrad.realize.realize(bn_beta.lazydata))
            mean_data = list(tinygrad.realize.realize(bn_mean.lazydata))
            var_data = list(tinygrad.realize.realize(bn_var.lazydata))
            w_data = list(tinygrad.realize.realize(conv_weight.lazydata))

            c_out = bn_gamma.shape[0]
            # Weight shape: (C_out, C_in_per_group, kH, kW) — total elements per channel
            elems_per_channel = len(w_data) // c_out

            if conv_bias is not None:
                b_data = list(tinygrad.realize.realize(conv_bias.lazydata))
            else:
                b_data = [0.0] * c_out

            # Compute fused parameters
            new_w = [0.0] * len(w_data)
            new_b = [0.0] * c_out

            for oc in range(c_out):
                # scale = gamma / sqrt(var + eps)
                scale = gamma_data[oc] / (var_data[oc] + eps) ** 0.5
                # new_weight[oc] = weight[oc] * scale
                base = oc * elems_per_channel
                for j in range(elems_per_channel):
                    new_w[base + j] = w_data[base + j] * scale
                # new_bias = (old_bias - mean) * scale + beta
                new_b[oc] = (b_data[oc] - mean_data[oc]) * scale + beta_data[oc]

            # Replace conv weight and bias in values
            self._values[conv_weight_name] = _make_tensor(new_w, conv_weight.shape)

            # Ensure conv node has a bias input
            fused_bias_name = conv_bias_name
            if fused_bias_name is None or fused_bias_name not in self._values:
                fused_bias_name = f"_fused_bias_{conv_idx}"
                # Extend conv inputs to include bias
                while len(conv_node["inputs"]) < 3:
                    conv_node["inputs"].append("")
                conv_node["inputs"][2] = fused_bias_name
            self._values[fused_bias_name] = _make_tensor(new_b, (c_out,))

            # Rewire: BN output now comes from Conv
            bn_output = node["outputs"][0] if node["outputs"] else None
            if bn_output:
                conv_node["outputs"][0] = bn_output
                # Update output_to_idx so later BN nodes can find the rewired Conv
                output_to_idx[bn_output] = conv_idx

            remove_indices.add(bn_idx)

        # Remove folded BN nodes (iterate in reverse to preserve indices)
        if remove_indices:
            self._graph_nodes = [n for i, n in enumerate(self._graph_nodes) if i not in remove_indices]

    def _eliminate_identity(self) -> None:
        """Remove Identity nodes by rewiring their input to their output."""
        new_nodes = []
        # Map: identity output name -> identity input name
        rewire: dict[str, str] = {}

        for node in self._graph_nodes:
            if node["op_type"] == "Identity":
                inp = node["inputs"][0] if node["inputs"] else ""
                out = node["outputs"][0] if node["outputs"] else ""
                if inp and out:
                    # Follow chains: if inp was itself rewired, use final target
                    while inp in rewire:
                        inp = rewire[inp]
                    rewire[out] = inp
                continue
            new_nodes.append(node)

        if not rewire:
            return

        # Rewrite all remaining nodes' inputs
        for node in new_nodes:
            node["inputs"] = [rewire.get(n, n) for n in node["inputs"]]

        self._graph_nodes = new_nodes

    # ------------------------------------------------------------------
    # Model loading
    # ------------------------------------------------------------------

    def _load_with_onnx(self, data: bytes) -> None:
        import onnx
        from onnx import numpy_helper
        import numpy as np

        model = onnx.load_from_string(data)
        graph = model.graph

        self._input_names = [inp.name for inp in graph.input]
        self._output_names = [out.name for out in graph.output]

        # Load initializers as constants
        for init in graph.initializer:
            arr = numpy_helper.to_array(init).astype(np.float32)
            shape = tuple(int(d) for d in arr.shape)
            values = arr.flatten().tolist()
            self._values[init.name] = _make_tensor(values, shape)

        # Walk nodes
        for node in graph.node:
            if node.op_type == "Constant":
                # Extract constant value and store
                name = node.output[0] if node.output else ""
                if not name:
                    continue
                for attr in node.attribute:
                    if attr.name == "value" and attr.t is not None:
                        t = attr.t
                        arr = numpy_helper.to_array(t)
                        # Keep int64 constants as int for shape ops
                        if t.data_type in (6, 7):
                            shape = tuple(int(d) for d in arr.shape) if arr.shape else (arr.size,)
                            values = arr.flatten().tolist()
                            self._values[name] = _make_int_tensor(values, shape)
                        else:
                            arr = arr.astype(np.float32)
                            shape = tuple(int(d) for d in arr.shape) if arr.shape else (arr.size,)
                            values = arr.flatten().tolist()
                            self._values[name] = _make_tensor(values, shape)
                    elif attr.name == "value_int":
                        self._values[name] = _make_int_tensor([attr.i], (1,))
                    elif attr.name == "value_float":
                        self._values[name] = _make_tensor([attr.f], (1,))
                continue

            # Parse attributes
            attrs: dict[str, object] = {}
            for attr in node.attribute:
                if attr.type == 1:  # FLOAT
                    attrs[attr.name] = attr.f
                elif attr.type == 2:  # INT
                    attrs[attr.name] = attr.i
                elif attr.type == 3:  # STRING
                    attrs[attr.name] = attr.s
                elif attr.type == 4:  # TENSOR
                    attrs[attr.name] = attr.t
                elif attr.type == 6:  # FLOATS
                    attrs[attr.name] = list(attr.floats)
                elif attr.type == 7:  # INTS
                    attrs[attr.name] = list(attr.ints)

            self._graph_nodes.append({
                "op_type": node.op_type,
                "inputs": list(node.input),
                "outputs": list(node.output),
                "attrs": attrs,
            })

    def _load_raw(self, data: bytes) -> None:
        """Weight-only raw protobuf parser.

        Uses the existing OnnxWeightParser for weight extraction and
        deliberately raises before graph execution. This path exists only
        to provide a clear error when the full ``onnx`` graph parser is
        unavailable.
        """
        from tinygrad.paddleocr import OnnxWeightParser
        parsed = OnnxWeightParser.parse(data)
        for name, (shape, dtype_code, values) in parsed.items():
            if not shape:
                shape = (len(values),)
            if dtype_code in (6, 7):
                self._values[name] = _make_int_tensor(values, shape)
            else:
                self._values[name] = _make_tensor(values, shape)

        # Without the full onnx library, we cannot extract the computation
        # graph (node list, I/O names). Raise so callers know to use the
        # onnx library path for full graph execution.
        raise RuntimeError(
            "Raw protobuf loader extracted weights but cannot parse the "
            "computation graph. Install the 'onnx' package for full graph "
            "execution."
        )


# ---------------------------------------------------------------------------
# Tensor construction helpers
# ---------------------------------------------------------------------------

def _make_tensor(values: list[float], shape: tuple[int, ...]) -> Tensor:
    """Create a float32 Tensor from flat values."""
    from tinygrad.lazy import LazyOp, LazyBuffer
    op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
    return Tensor(LazyBuffer(op, dtypes.float32, shape, data=values))


def _make_int_tensor(values: list[int], shape: tuple[int, ...]) -> Tensor:
    """Create an int64 Tensor from flat values (for shape/index constants)."""
    from tinygrad.lazy import LazyOp, LazyBuffer
    op = LazyOp("LOAD", (), dtype=dtypes.int64, shape=shape)
    return Tensor(LazyBuffer(op, dtypes.int64, shape, data=values))


def _realize_ints(t: Tensor) -> list[int]:
    """Realize a tensor and return its values as a list of ints."""
    import tinygrad.realize
    flat = tinygrad.realize.realize(t.lazydata)
    return [int(x) for x in flat]


def _realize_floats(t: Tensor) -> list[float]:
    """Realize a tensor and return its values as a list of floats."""
    import tinygrad.realize
    return list(tinygrad.realize.realize(t.lazydata))


# ---------------------------------------------------------------------------
# ONNX Op implementations — each returns a list of output Tensors
# ---------------------------------------------------------------------------

def _broadcast_pair(a: Tensor, b: Tensor) -> tuple[Tensor, Tensor]:
    """Broadcast two tensors to a common shape (ONNX broadcasting rules).

    Handles cases where either tensor may be smaller, including scalar
    tensors and tensors with different numbers of dimensions.
    """
    if a.shape == b.shape:
        return a, b

    # Compute broadcast shape following numpy/ONNX rules
    a_shape = list(a.shape)
    b_shape = list(b.shape)
    ndim = max(len(a_shape), len(b_shape))
    # Left-pad shorter shape with 1s
    while len(a_shape) < ndim:
        a_shape.insert(0, 1)
    while len(b_shape) < ndim:
        b_shape.insert(0, 1)

    out_shape = []
    for da, db in zip(a_shape, b_shape):
        if da == db:
            out_shape.append(da)
        elif da == 1:
            out_shape.append(db)
        elif db == 1:
            out_shape.append(da)
        else:
            raise ValueError(f"Cannot broadcast shapes {a.shape} and {b.shape}")

    out = tuple(out_shape)

    # Reshape to match ndim if needed, then broadcast
    if a.shape != out:
        a = a.reshape(*a_shape)._broadcast_to(out)
    if b.shape != out:
        b = b.reshape(*b_shape)._broadcast_to(out)

    return a, b


def _op_add(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    a, b = _broadcast_pair(inputs[0], inputs[1])
    return [a + b]


def _op_sub(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    a, b = _broadcast_pair(inputs[0], inputs[1])
    return [a + b * (-1.0)]


def _op_mul(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    a, b = _broadcast_pair(inputs[0], inputs[1])
    return [a * b]


def _op_div(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    a, b = _broadcast_pair(inputs[0], inputs[1])
    return [a * b.reciprocal()]


def _op_pow(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    base, exp_t = inputs[0], inputs[1]
    # For ONNX Pow, the common case is x^2 (LayerNorm variance).
    # General: x^y = exp2(y * log2(x)).  Handles y=2 via x*x shortcut.
    exp_vals = _realize_floats(exp_t)
    if len(exp_vals) == 1 and exp_vals[0] == 2.0:
        return [base * base]
    if len(exp_vals) == 1 and exp_vals[0] == 0.5:
        return [base.sqrt()]
    if len(exp_vals) == 1 and exp_vals[0] == 3.0:
        return [base * base * base]
    # General path: x^y = exp2(y * log2(|x|))
    # Uses |x| since log2 is undefined for negative values.
    # Neural net pow ops almost always have non-negative bases (variance, etc.)
    abs_base = base.relu() + (base * (-1.0)).relu()  # |x| = relu(x) + relu(-x)
    # Add small epsilon to avoid log2(0)
    log_base = (abs_base + 1e-12).log2()
    scaled = log_base * exp_t._broadcast_to(base.shape)
    return [scaled.exp2()]


def _op_sqrt(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    return [inputs[0].sqrt()]


def _op_relu(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    return [inputs[0].relu()]


def _op_sigmoid(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    return [inputs[0].sigmoid()]


def _op_clip(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """Clip(x, min, max).  In opset < 11, min/max are attributes."""
    x = inputs[0]
    # Opset >= 11: min/max are inputs[1], inputs[2]
    # Opset < 11: min/max are attributes
    min_val = _get_attr_float(attrs, "min", float("-inf"))
    max_val = _get_attr_float(attrs, "max", float("inf"))

    if len(inputs) >= 2 and inputs[1] is not None:
        min_vals = _realize_floats(inputs[1])
        min_val = min_vals[0] if min_vals else float("-inf")
    if len(inputs) >= 3 and inputs[2] is not None:
        max_vals = _realize_floats(inputs[2])
        max_val = max_vals[0] if max_vals else float("inf")

    # clip(x, lo, hi) = min(max(x, lo), hi)
    # max(x, lo) = relu(x - lo) + lo
    # min(x, hi) = hi - relu(hi - x) = -(relu(hi - x)) + hi
    result = x
    if min_val != float("-inf"):
        # max(x, min_val) = relu(x - min_val) + min_val
        result = (result + (-min_val)).relu() + min_val
    if max_val != float("inf"):
        # min(result, max_val) = max_val - relu(max_val - result)
        result = (result * (-1.0) + max_val).relu() * (-1.0) + max_val
    return [result]


def _op_hard_sigmoid(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """HardSigmoid: clip(alpha * x + beta, 0, 1).

    ONNX default: alpha=0.2, beta=0.5.
    PaddleOCR uses alpha=0.1666..., beta=0.5 (equivalent to (x+3)/6 clipped).
    """
    x = inputs[0]
    alpha = _get_attr_float(attrs, "alpha", 0.2)
    beta = _get_attr_float(attrs, "beta", 0.5)
    # clip(alpha*x + beta, 0, 1) = relu(alpha*x + beta) - relu(alpha*x + beta - 1)
    ax_b = x * alpha + beta
    return [ax_b.relu() + (ax_b + (-1.0)).relu() * (-1.0)]


def _op_hard_swish(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """HardSwish: x * clip(x/6 + 0.5, 0, 1) = x * HardSigmoid(x, alpha=1/6, beta=0.5).

    Used extensively in MobileNetV3-based PaddleOCR recognizers.
    """
    x = inputs[0]
    # HardSigmoid with alpha=1/6, beta=0.5: clip(x/6 + 0.5, 0, 1)
    alpha = 1.0 / 6.0
    beta = 0.5
    ax_b = x * alpha + beta
    hs = ax_b.relu() + (ax_b + (-1.0)).relu() * (-1.0)  # clip(ax_b, 0, 1)
    return [x * hs]


def _op_softmax(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    axis = _get_attr_int(attrs, "axis", -1)
    return [inputs[0].softmax(axis=axis)]


def _op_reduce_mean(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    x = inputs[0]
    axes = _get_attr_ints(attrs, "axes", [])
    keepdims = _get_attr_int(attrs, "keepdims", 1)

    # If axes provided as input (opset 18+)
    if not axes and len(inputs) >= 2 and inputs[1] is not None:
        axes = _realize_ints(inputs[1])

    if not axes:
        # Reduce all
        axes = list(range(len(x.shape)))

    # Normalize negative axes
    ndim = len(x.shape)
    axes = [a if a >= 0 else a + ndim for a in axes]
    axes = sorted(axes)

    result = x
    count = 1
    for ax in reversed(axes):
        count *= result.shape[ax]
        result = result.sum(axis=ax)
        if keepdims:
            shape_list = list(result.shape)
            shape_list.insert(ax, 1)
            result = result.reshape(*shape_list)

    return [result * (1.0 / count)]


def _op_global_avg_pool(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """GlobalAveragePool: (N,C,H,W) -> (N,C,1,1)."""
    x = inputs[0]
    n, c = x.shape[0], x.shape[1]
    spatial = 1
    for d in x.shape[2:]:
        spatial *= d
    # Flatten spatial dims, sum, divide
    flat = x.reshape(n, c, spatial)
    summed = flat.sum(axis=-1)  # (N, C)
    result = summed * (1.0 / spatial)
    return [result.reshape(n, c, 1, 1)]


def _op_average_pool(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """AveragePool with kernel_shape, strides, pads."""
    x = inputs[0]
    kernel_shape = _get_attr_ints(attrs, "kernel_shape", [1, 1])
    strides = _get_attr_ints(attrs, "strides", [1, 1])
    pads = _get_attr_ints(attrs, "pads", [0, 0, 0, 0])
    kh, kw = kernel_shape[0], kernel_shape[1]
    sh, sw = strides[0], strides[1]
    pad_top, pad_left = pads[0], pads[1]
    # pads[2], pads[3] are bottom, right

    import tinygrad.realize
    flat = tinygrad.realize.realize(x.lazydata)
    n, c, h, w = x.shape

    # Apply padding
    h_padded = h + pads[0] + pads[2]
    w_padded = w + pads[1] + pads[3]

    oh = (h_padded - kh) // sh + 1
    ow = (w_padded - kw) // sw + 1

    out_size = n * c * oh * ow
    result = [0.0] * out_size
    pool_area = kh * kw

    for bn in range(n):
        for ch in range(c):
            for oy in range(oh):
                for ox in range(ow):
                    s = 0.0
                    for fy in range(kh):
                        for fx in range(kw):
                            iy = oy * sh + fy - pad_top
                            ix = ox * sw + fx - pad_left
                            if 0 <= iy < h and 0 <= ix < w:
                                idx = bn * (c * h * w) + ch * (h * w) + iy * w + ix
                                s += flat[idx]
                    out_idx = bn * (c * oh * ow) + ch * (oh * ow) + oy * ow + ox
                    result[out_idx] = s / pool_area

    return [_make_tensor(result, (n, c, oh, ow))]


def _op_conv(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """Conv2d supporting groups (depthwise, grouped, standard).

    Handles:
      - group=1: standard convolution
      - group=C_in: depthwise separable convolution
      - group=G: grouped convolution (split channels into G groups)
    """
    x = inputs[0]
    weight = inputs[1]
    bias = inputs[2] if len(inputs) > 2 and inputs[2] is not None else None

    group = _get_attr_int(attrs, "group", 1)
    strides = _get_attr_ints(attrs, "strides", [1, 1])
    pads = _get_attr_ints(attrs, "pads", [0, 0, 0, 0])
    dilations = _get_attr_ints(attrs, "dilations", [1, 1])

    stride_h, stride_w = strides[0], strides[1] if len(strides) > 1 else strides[0]
    pad_top, pad_left = pads[0], pads[1] if len(pads) > 1 else pads[0]
    pad_bottom = pads[2] if len(pads) > 2 else pad_top
    pad_right = pads[3] if len(pads) > 3 else pad_left
    dil_h, dil_w = dilations[0], dilations[1] if len(dilations) > 1 else dilations[0]

    if group == 1 and dil_h == 1 and dil_w == 1 and pad_top == pad_bottom and pad_left == pad_right:
        # Standard conv — use Tensor.conv2d (faster im2col path)
        result = Tensor.conv2d(x, weight, bias, stride=stride_h, padding=pad_top)
        return [result]

    # General grouped conv with arbitrary padding/dilation
    return [_grouped_conv2d(x, weight, bias, group, stride_h, stride_w,
                            pad_top, pad_left, pad_bottom, pad_right,
                            dil_h, dil_w)]


def _grouped_conv2d(x: Tensor, weight: Tensor, bias: Tensor | None,
                    groups: int, stride_h: int, stride_w: int,
                    pad_top: int, pad_left: int, pad_bottom: int, pad_right: int,
                    dil_h: int, dil_w: int) -> Tensor:
    """Grouped conv2d via im2col + matmul, supporting dilation and asymmetric padding."""
    import tinygrad.realize

    x_data = tinygrad.realize.realize(x.lazydata)
    w_data = tinygrad.realize.realize(weight.lazydata)

    n, c_in, h, w_dim = x.shape
    c_out, c_in_per_group, kh, kw = weight.shape

    # Effective kernel size with dilation
    eff_kh = (kh - 1) * dil_h + 1
    eff_kw = (kw - 1) * dil_w + 1

    h_padded = h + pad_top + pad_bottom
    w_padded = w_dim + pad_left + pad_right
    h_out = (h_padded - eff_kh) // stride_h + 1
    w_out = (w_padded - eff_kw) // stride_w + 1

    c_out_per_group = c_out // groups
    c_in_per_grp = c_in // groups

    out_size = n * c_out * h_out * w_out
    result = [0.0] * out_size

    for bn in range(n):
        for g in range(groups):
            c_in_start = g * c_in_per_grp
            c_out_start = g * c_out_per_group

            for oc in range(c_out_per_group):
                oc_abs = c_out_start + oc
                for oh in range(h_out):
                    for ow in range(w_out):
                        s = 0.0
                        for ic in range(c_in_per_grp):
                            ic_abs = c_in_start + ic
                            for fh in range(kh):
                                for fw in range(kw):
                                    ih = oh * stride_h - pad_top + fh * dil_h
                                    iw = ow * stride_w - pad_left + fw * dil_w
                                    if 0 <= ih < h and 0 <= iw < w_dim:
                                        x_idx = (bn * (c_in * h * w_dim) +
                                                 ic_abs * (h * w_dim) +
                                                 ih * w_dim + iw)
                                        w_idx = (oc_abs * (c_in_per_grp * kh * kw) +
                                                 ic * (kh * kw) +
                                                 fh * kw + fw)
                                        s += x_data[x_idx] * w_data[w_idx]
                        out_idx = (bn * (c_out * h_out * w_out) +
                                   oc_abs * (h_out * w_out) +
                                   oh * w_out + ow)
                        result[out_idx] = s

    # Add bias
    if bias is not None:
        b_data = tinygrad.realize.realize(bias.lazydata)
        for bn in range(n):
            for oc in range(c_out):
                for oh in range(h_out):
                    for ow in range(w_out):
                        idx = (bn * (c_out * h_out * w_out) +
                               oc * (h_out * w_out) + oh * w_out + ow)
                        result[idx] += b_data[oc]

    return _make_tensor(result, (n, c_out, h_out, w_out))


def _op_conv_transpose(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """ConvTranspose (transposed/deconvolution) — used in DB head for upsampling."""
    x = inputs[0]
    weight = inputs[1]
    bias = inputs[2] if len(inputs) > 2 and inputs[2] is not None else None

    group = _get_attr_int(attrs, "group", 1)
    strides = _get_attr_ints(attrs, "strides", [1, 1])
    pads = _get_attr_ints(attrs, "pads", [0, 0, 0, 0])
    output_padding = _get_attr_ints(attrs, "output_padding", [0, 0])

    stride_h, stride_w = strides[0], strides[1]
    pad_top, pad_left = pads[0], pads[1]
    pad_bottom = pads[2] if len(pads) > 2 else pad_top
    pad_right = pads[3] if len(pads) > 3 else pad_left

    import tinygrad.realize
    x_data = tinygrad.realize.realize(x.lazydata)
    w_data = tinygrad.realize.realize(weight.lazydata)

    n, c_in, h_in, w_in = x.shape
    c_in_w, c_out_per_group, kh, kw = weight.shape
    c_out = c_out_per_group * group

    h_out = (h_in - 1) * stride_h - pad_top - pad_bottom + kh + output_padding[0]
    w_out = (w_in - 1) * stride_w - pad_left - pad_right + kw + output_padding[1]

    c_in_per_group = c_in // group
    out_size = n * c_out * h_out * w_out
    result = [0.0] * out_size

    for bn in range(n):
        for g in range(group):
            for ic in range(c_in_per_group):
                ic_abs = g * c_in_per_group + ic
                for oc in range(c_out_per_group):
                    oc_abs = g * c_out_per_group + oc
                    for ih in range(h_in):
                        for iw in range(w_in):
                            x_val = x_data[bn * (c_in * h_in * w_in) +
                                           ic_abs * (h_in * w_in) +
                                           ih * w_in + iw]
                            if x_val == 0.0:
                                continue
                            for fh in range(kh):
                                for fw in range(kw):
                                    oh = ih * stride_h + fh - pad_top
                                    ow = iw * stride_w + fw - pad_left
                                    if 0 <= oh < h_out and 0 <= ow < w_out:
                                        w_idx = (ic_abs * (c_out_per_group * kh * kw) +
                                                 oc * (kh * kw) + fh * kw + fw)
                                        out_idx = (bn * (c_out * h_out * w_out) +
                                                   oc_abs * (h_out * w_out) +
                                                   oh * w_out + ow)
                                        result[out_idx] += x_val * w_data[w_idx]

    if bias is not None:
        b_data = tinygrad.realize.realize(bias.lazydata)
        for bn in range(n):
            for oc in range(c_out):
                for oh in range(h_out):
                    for ow in range(w_out):
                        idx = (bn * (c_out * h_out * w_out) +
                               oc * (h_out * w_out) + oh * w_out + ow)
                        result[idx] += b_data[oc]

    return [_make_tensor(result, (n, c_out, h_out, w_out))]


def _op_matmul(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    return [inputs[0].matmul(inputs[1])]


def _op_batch_norm(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """BatchNormalization: (x - mean) / sqrt(var + eps) * scale + bias."""
    x = inputs[0]
    scale = inputs[1]
    bias = inputs[2]
    mean = inputs[3]
    var = inputs[4]
    eps = _get_attr_float(attrs, "epsilon", 1e-5)

    # Reshape to (1, C, 1, 1) for broadcasting
    c = scale.shape[0]
    ndim = len(x.shape)
    if ndim == 4:
        reshape_dims = (1, c, 1, 1)
    elif ndim == 3:
        reshape_dims = (1, c, 1)
    elif ndim == 2:
        reshape_dims = (1, c)
    else:
        reshape_dims = (c,)

    s = scale.reshape(*reshape_dims)
    b = bias.reshape(*reshape_dims)
    m = mean.reshape(*reshape_dims)
    v = var.reshape(*reshape_dims)

    inv_std = (v + eps).sqrt().reciprocal()
    result = (x + m._broadcast_to(x.shape) * (-1.0)) * inv_std._broadcast_to(x.shape) * s._broadcast_to(x.shape) + b._broadcast_to(x.shape)
    return [result]


def _op_reshape(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    x = inputs[0]
    if len(inputs) >= 2 and inputs[1] is not None:
        shape = _realize_ints(inputs[1])
    else:
        shape = _get_attr_ints(attrs, "shape", [])

    if not shape:
        return [x]

    # Handle 0 dims (copy from input shape)
    allowzero = _get_attr_int(attrs, "allowzero", 0)
    if not allowzero:
        for i in range(len(shape)):
            if shape[i] == 0 and i < len(x.shape):
                shape[i] = x.shape[i]

    return [x.reshape(*shape)]


def _op_transpose(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    x = inputs[0]
    perm = _get_attr_ints(attrs, "perm", [])
    if not perm:
        # Default: reverse all axes
        perm = list(range(len(x.shape) - 1, -1, -1))
    return [x.permute(*perm)]


def _op_squeeze(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    x = inputs[0]
    axes = _get_attr_ints(attrs, "axes", [])
    if not axes and len(inputs) >= 2 and inputs[1] is not None:
        axes = _realize_ints(inputs[1])

    if not axes:
        # Squeeze all dims of size 1
        new_shape = [d for d in x.shape if d != 1]
        if not new_shape:
            new_shape = [1]
        return [x.reshape(*new_shape)]

    # Normalize negative axes
    ndim = len(x.shape)
    axes = sorted([a if a >= 0 else a + ndim for a in axes], reverse=True)
    new_shape = list(x.shape)
    for ax in axes:
        if new_shape[ax] == 1:
            new_shape.pop(ax)
    if not new_shape:
        new_shape = [1]
    return [x.reshape(*new_shape)]


def _op_unsqueeze(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    x = inputs[0]
    axes = _get_attr_ints(attrs, "axes", [])
    if not axes and len(inputs) >= 2 and inputs[1] is not None:
        axes = _realize_ints(inputs[1])

    new_shape = list(x.shape)
    for ax in sorted(axes):
        if ax < 0:
            ax = len(new_shape) + 1 + ax
        new_shape.insert(ax, 1)
    return [x.reshape(*new_shape)]


def _op_concat(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    axis = _get_attr_int(attrs, "axis", 0)
    tensors = [t for t in inputs if t is not None]
    return [Tensor.cat(*tensors, dim=axis)]


def _op_slice(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """Slice(data, starts, ends, axes?, steps?)."""
    x = inputs[0]
    starts = _realize_ints(inputs[1]) if len(inputs) > 1 and inputs[1] is not None else []
    ends = _realize_ints(inputs[2]) if len(inputs) > 2 and inputs[2] is not None else []
    axes = _realize_ints(inputs[3]) if len(inputs) > 3 and inputs[3] is not None else list(range(len(starts)))
    steps = _realize_ints(inputs[4]) if len(inputs) > 4 and inputs[4] is not None else [1] * len(starts)

    ndim = len(x.shape)
    bounds = [(0, d) for d in x.shape]

    for i, ax in enumerate(axes):
        if ax < 0:
            ax = ndim + ax
        s = starts[i]
        e = ends[i]
        step = steps[i] if i < len(steps) else 1

        dim_size = x.shape[ax]

        # Clamp start
        if s < 0:
            s = max(0, s + dim_size)
        s = min(s, dim_size)

        # Clamp end
        if e < 0:
            e = max(0, e + dim_size)
        # Handle very large end values (INT64_MAX sentinel)
        e = min(e, dim_size)

        if step != 1:
            # Step != 1 requires more complex extraction.
            # For PaddleOCR, step is always 1 in practice.
            pass

        bounds[ax] = (s, e)

    return [x.shrink(bounds)]


def _op_shape(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """Shape(data) -> int64 tensor of shape dimensions."""
    x = inputs[0]
    shape_vals = list(x.shape)
    return [_make_int_tensor(shape_vals, (len(shape_vals),))]


def _op_cast(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """Cast: type coercion. For tinygrad, we keep float32 for compute tensors
    and int64 for shape tensors. The cast is a passthrough for neural net inference."""
    x = inputs[0]
    to_type = _get_attr_int(attrs, "to", 1)  # 1=float32
    # In practice, PaddleOCR casts are float->float or int->int
    # For shape tensors that get cast to float for arithmetic, realize and convert
    if to_type in (1, 11):  # float32, double
        if x.dtype == dtypes.int64 or x.dtype == dtypes.int32:
            vals = _realize_ints(x)
            return [_make_tensor([float(v) for v in vals], x.shape)]
    elif to_type in (6, 7):  # int32, int64
        if x.dtype == dtypes.float32:
            vals = _realize_floats(x)
            return [_make_int_tensor([int(v) for v in vals], x.shape)]
    return [x]


def _op_identity(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    return [inputs[0]]


def _op_resize(inputs: list[Tensor | None], attrs: dict) -> list[Tensor]:
    """Resize: nearest-neighbor upsampling.

    ONNX Resize has multiple input variants:
      - inputs[0]: X
      - inputs[1]: roi (unused for nearest)
      - inputs[2]: scales OR
      - inputs[3]: sizes
    """
    x = inputs[0]
    mode = _get_attr_string(attrs, "mode", "nearest")

    # Determine output size from scales or sizes input
    scales = None
    sizes = None

    if len(inputs) >= 3 and inputs[2] is not None:
        scale_vals = _realize_floats(inputs[2])
        if scale_vals and any(s != 0.0 for s in scale_vals):
            scales = scale_vals

    if len(inputs) >= 4 and inputs[3] is not None:
        sizes = _realize_ints(inputs[3])

    if sizes:
        target_shape = tuple(sizes)
    elif scales:
        target_shape = tuple(int(d * s) for d, s in zip(x.shape, scales))
    else:
        # No resize info — return as-is
        return [x]

    # Only handle 4D (N,C,H,W) nearest-neighbor resize
    if len(x.shape) != 4 or mode != "nearest":
        return [x]

    return [_nearest_resize(x, target_shape)]


def _nearest_resize(x: Tensor, target_shape: tuple[int, ...]) -> Tensor:
    """Nearest-neighbor resize for 4D tensors."""
    import tinygrad.realize

    flat = tinygrad.realize.realize(x.lazydata)
    n, c, h, w = x.shape
    _, _, th, tw = target_shape

    out_size = n * c * th * tw
    result = [0.0] * out_size

    for bn in range(n):
        for ch in range(c):
            for oy in range(th):
                for ox in range(tw):
                    # Nearest neighbor: floor(oy * h / th)
                    src_y = min(oy * h // th, h - 1)
                    src_x = min(ox * w // tw, w - 1)
                    src_idx = bn * (c * h * w) + ch * (h * w) + src_y * w + src_x
                    dst_idx = bn * (c * th * tw) + ch * (th * tw) + oy * tw + ox
                    result[dst_idx] = flat[src_idx]

    return _make_tensor(result, target_shape)


# ---------------------------------------------------------------------------
# Op dispatch table
# ---------------------------------------------------------------------------

_OP_DISPATCH: dict[str, object] = {
    "Add": _op_add,
    "Sub": _op_sub,
    "Mul": _op_mul,
    "Div": _op_div,
    "Pow": _op_pow,
    "Sqrt": _op_sqrt,
    "Relu": _op_relu,
    "Sigmoid": _op_sigmoid,
    "Clip": _op_clip,
    "HardSigmoid": _op_hard_sigmoid,
    "HardSwish": _op_hard_swish,
    "Softmax": _op_softmax,
    "ReduceMean": _op_reduce_mean,
    "GlobalAveragePool": _op_global_avg_pool,
    "AveragePool": _op_average_pool,
    "Conv": _op_conv,
    "ConvTranspose": _op_conv_transpose,
    "MatMul": _op_matmul,
    "BatchNormalization": _op_batch_norm,
    "Reshape": _op_reshape,
    "Transpose": _op_transpose,
    "Squeeze": _op_squeeze,
    "Unsqueeze": _op_unsqueeze,
    "Concat": _op_concat,
    "Slice": _op_slice,
    "Shape": _op_shape,
    "Cast": _op_cast,
    "Identity": _op_identity,
    "Resize": _op_resize,
}
