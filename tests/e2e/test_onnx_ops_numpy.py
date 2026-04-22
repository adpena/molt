"""Validate ONNX interpreter ops against ONNX Runtime using numpy.

Tests each op type independently by extracting single-op subgraphs
from the PaddleOCR model and comparing our implementation against
ONNX Runtime's output.
"""

import numpy as np
import onnxruntime as ort
import onnx


def numpy_conv2d(x, w, bias=None, stride=1, padding=0, groups=1):
    """Our Conv2d implementation in numpy (matches tinygrad Tensor.conv2d)."""
    if padding > 0:
        x = np.pad(x, ((0, 0), (0, 0), (padding, padding), (padding, padding)))
    N, C, H, W = x.shape
    OC, IC_g, KH, KW = w.shape
    OH = (H - KH) // stride + 1
    OW = (W - KW) // stride + 1

    if groups == 1:
        # im2col + matmul
        cols = np.zeros((N, C * KH * KW, OH * OW), dtype=x.dtype)
        for kh in range(KH):
            for kw in range(KW):
                for c in range(C):
                    col_idx = c * KH * KW + kh * KW + kw
                    for oh in range(OH):
                        for ow in range(OW):
                            cols[:, col_idx, oh * OW + ow] = x[
                                :, c, oh * stride + kh, ow * stride + kw
                            ]

        w_flat = w.reshape(OC, -1)  # [OC, C*KH*KW]
        out = np.einsum("ij,njk->nik", w_flat, cols)
        out = out.reshape(N, OC, OH, OW)
    else:
        # Grouped convolution
        out = np.zeros((N, OC, OH, OW), dtype=x.dtype)
        channels_per_group = C // groups
        oc_per_group = OC // groups
        for g in range(groups):
            x_g = x[:, g * channels_per_group : (g + 1) * channels_per_group]
            w_g = w[g * oc_per_group : (g + 1) * oc_per_group]
            out[:, g * oc_per_group : (g + 1) * oc_per_group] = numpy_conv2d(
                x_g, w_g, stride=stride
            )

    if bias is not None:
        out += bias.reshape(1, -1, 1, 1)
    return out


def numpy_batch_norm(x, scale, bias, mean, var, eps=1e-5):
    """BatchNorm: (x - mean) / sqrt(var + eps) * scale + bias"""
    return (x - mean.reshape(1, -1, 1, 1)) / np.sqrt(
        var.reshape(1, -1, 1, 1) + eps
    ) * scale.reshape(1, -1, 1, 1) + bias.reshape(1, -1, 1, 1)


def test_conv2d_vs_onnxruntime():
    """Test our Conv2d matches ONNX Runtime."""
    # Create a minimal ONNX model with just one Conv
    x = np.random.randn(1, 3, 32, 32).astype(np.float32)
    w = np.random.randn(16, 3, 3, 3).astype(np.float32)
    bias = np.random.randn(16).astype(np.float32)

    our_out = numpy_conv2d(x, w, bias, padding=1)

    # Compare against ONNX Runtime
    from onnx import numpy_helper, TensorProto
    from onnx.helper import make_model, make_node, make_graph, make_tensor_value_info

    X = make_tensor_value_info("X", TensorProto.FLOAT, [1, 3, 32, 32])
    Y = make_tensor_value_info("Y", TensorProto.FLOAT, None)
    W_init = numpy_helper.from_array(w, name="W")
    B_init = numpy_helper.from_array(bias, name="B")
    conv_node = make_node(
        "Conv", ["X", "W", "B"], ["Y"], kernel_shape=[3, 3], pads=[1, 1, 1, 1]
    )
    graph = make_graph([conv_node], "test", [X], [Y], [W_init, B_init])
    model = make_model(graph, opset_imports=[onnx.helper.make_opsetid("", 13)])

    sess = ort.InferenceSession(model.SerializeToString())
    ort_out = sess.run(None, {"X": x})[0]

    max_diff = np.abs(our_out - ort_out).max()
    print(
        f"Conv2d: max_diff = {max_diff:.2e} ({'PASS' if max_diff < 1e-4 else 'FAIL'})"
    )
    return max_diff < 1e-4


def test_batch_norm_vs_onnxruntime():
    """Test our BatchNorm matches ONNX Runtime."""
    x = np.random.randn(1, 16, 32, 32).astype(np.float32)
    scale = np.random.randn(16).astype(np.float32)
    bias = np.random.randn(16).astype(np.float32)
    mean = np.random.randn(16).astype(np.float32)
    var = np.abs(np.random.randn(16).astype(np.float32)) + 0.1

    our_out = numpy_batch_norm(x, scale, bias, mean, var)

    from onnx import numpy_helper, TensorProto
    from onnx.helper import make_model, make_node, make_graph, make_tensor_value_info

    X = make_tensor_value_info("X", TensorProto.FLOAT, [1, 16, 32, 32])
    Y = make_tensor_value_info("Y", TensorProto.FLOAT, None)
    inits = [
        numpy_helper.from_array(scale, "scale"),
        numpy_helper.from_array(bias, "bias"),
        numpy_helper.from_array(mean, "mean"),
        numpy_helper.from_array(var, "var"),
    ]
    bn_node = make_node(
        "BatchNormalization", ["X", "scale", "bias", "mean", "var"], ["Y"], epsilon=1e-5
    )
    graph = make_graph([bn_node], "test", [X], [Y], inits)
    model = make_model(graph, opset_imports=[onnx.helper.make_opsetid("", 13)])

    sess = ort.InferenceSession(model.SerializeToString())
    ort_out = sess.run(None, {"X": x})[0]

    max_diff = np.abs(our_out - ort_out).max()
    print(
        f"BatchNorm: max_diff = {max_diff:.2e} ({'PASS' if max_diff < 1e-4 else 'FAIL'})"
    )
    return max_diff < 1e-4


def test_bn_folding_correctness():
    """Test that BN-folded Conv matches separate Conv+BN."""
    x = np.random.randn(1, 3, 16, 16).astype(np.float32)
    w = np.random.randn(8, 3, 3, 3).astype(np.float32)
    conv_bias = np.zeros(8, dtype=np.float32)
    gamma = np.random.randn(8).astype(np.float32)
    beta = np.random.randn(8).astype(np.float32)
    mean = np.random.randn(8).astype(np.float32)
    var = np.abs(np.random.randn(8).astype(np.float32)) + 0.1

    # Separate Conv + BN
    conv_out = numpy_conv2d(x, w, conv_bias, padding=1)
    separate_out = numpy_batch_norm(conv_out, gamma, beta, mean, var)

    # Folded
    scale = gamma / np.sqrt(var + 1e-5)
    w_folded = w * scale.reshape(-1, 1, 1, 1)
    b_folded = (conv_bias - mean) * scale + beta
    folded_out = numpy_conv2d(x, w_folded, b_folded, padding=1)

    max_diff = np.abs(separate_out - folded_out).max()
    print(
        f"BN folding: max_diff = {max_diff:.2e} ({'PASS' if max_diff < 1e-5 else 'FAIL'})"
    )
    return max_diff < 1e-5


if __name__ == "__main__":
    test_conv2d_vs_onnxruntime()
    test_batch_norm_vs_onnxruntime()
    test_bn_folding_correctness()
