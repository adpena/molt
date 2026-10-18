"""Print the ONNX op inventory and ONNX Runtime timing of the PaddleOCR models.

A manual measurement script, not a test: it calls no Molt code and asserts
nothing. docs/benchmarks/paddleocr_baseline.md cites its output. It needs
onnx, onnxruntime and numpy, which no locked dependency group provides, and
the PaddleOCR models under the root that tests/helpers/paddleocr_paths.py
names.

Usage: python -m tests.e2e.paddleocr_onnx_inventory
"""

import time
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort

from tests.helpers.paddleocr_paths import (
    PADDLEOCR_MODEL_ROOT_ENV,
    hf_snapshot_artifact_candidate,
    paddleocr_model_root,
)


def require_paddleocr_artifact(*relative_candidates: str) -> Path:
    root = paddleocr_model_root().expanduser()
    for relative in relative_candidates:
        candidate = root / relative
        if candidate.is_file():
            return candidate
    raise SystemExit(
        f"no PaddleOCR artifact under {root} ({PADDLEOCR_MODEL_ROOT_ENV}); "
        f"tried: {', '.join(relative_candidates)}"
    )


DETECTOR_CANDIDATES = (
    "ch_PP-OCRv4_det.onnx",
    hf_snapshot_artifact_candidate(
        "models--OleehyO--paddleocrv4.onnx", "ch_PP-OCRv4_det.onnx"
    ),
)
ENGLISH_RECOGNIZER_CANDIDATES = (
    "rec/english/model.onnx",
    hf_snapshot_artifact_candidate(
        "models--Kreuzberg--paddleocr-onnx-models", "rec/english/model.onnx"
    ),
)


def detector_inventory():
    """Print the detector's op inventory and ONNX Runtime timing."""
    det_path = require_paddleocr_artifact(*DETECTOR_CANDIDATES)
    model = onnx.load(det_path)

    # ONNX Runtime reference
    sess = ort.InferenceSession(str(det_path))
    inp = np.random.randn(1, 3, 256, 320).astype(np.float32)
    ref_out = sess.run(None, {sess.get_inputs()[0].name: inp})[0]

    # Verify graph structure for our interpreter
    ops = {}
    for node in model.graph.node:
        ops[node.op_type] = ops.get(node.op_type, 0) + 1

    supported = {
        "Conv",
        "BatchNormalization",
        "Relu",
        "Add",
        "Mul",
        "Div",
        "Sigmoid",
        "HardSigmoid",
        "HardSwish",
        "Clip",
        "Reshape",
        "Transpose",
        "Squeeze",
        "Unsqueeze",
        "Concat",
        "Slice",
        "GlobalAveragePool",
        "Resize",
        "Shape",
        "Cast",
        "Identity",
        "Constant",
        "MatMul",
        "Softmax",
        "ReduceMean",
        "Pow",
        "Sqrt",
        "Sub",
        "AveragePool",
        "ConvTranspose",
    }

    unsupported = set(ops.keys()) - supported
    print(
        f"Reference output: shape={ref_out.shape} range=[{ref_out.min():.4f}, {ref_out.max():.4f}]"
    )
    print(f"Ops needed: {sorted(ops.keys())}")
    print(f"Unsupported: {unsupported or 'NONE — all ops supported!'}")
    print(f"Conv nodes: {ops.get('Conv', 0)}")
    print(f"BatchNorm nodes: {ops.get('BatchNormalization', 0)}")
    print(
        f"BN folding candidates: {min(ops.get('Conv', 0), ops.get('BatchNormalization', 0))}"
    )

    # Profile ONNX Runtime per-op (approximate by running subsets)
    times = []
    for _ in range(10):
        start = time.time()
        sess.run(None, {sess.get_inputs()[0].name: inp})
        times.append(time.time() - start)
    print(
        f"\nONNX Runtime: {np.mean(times) * 1000:.1f} ms avg ({np.min(times) * 1000:.1f} min)"
    )


def recognizer_inventory():
    """Print the English recognizer's output shape and op inventory."""
    rec_path = require_paddleocr_artifact(*ENGLISH_RECOGNIZER_CANDIDATES)

    sess = ort.InferenceSession(str(rec_path))
    inp = np.random.randn(1, 3, 48, 200).astype(np.float32)
    ref_out = sess.run(None, {sess.get_inputs()[0].name: inp})[0]

    model = onnx.load(rec_path)
    ops = {}
    for node in model.graph.node:
        ops[node.op_type] = ops.get(node.op_type, 0) + 1

    print(f"\nRecognizer output: shape={ref_out.shape}")
    print(f"Ops: {sorted(ops.keys())}")


if __name__ == "__main__":
    detector_inventory()
    recognizer_inventory()
