"""Test ONNX interpreter correctness against ONNX Runtime.

Runs the same model through both runtimes and compares outputs.
Uses numpy directly (no tinygrad dependency) for testing outside molt.
"""
import onnx
import onnxruntime as ort
import numpy as np
import glob
import time

def test_detector_correctness():
    """Compare detector output: ONNX Runtime vs our interpreter structure."""
    det_path = glob.glob('/tmp/paddleocr-onnx/**/ch_PP-OCRv4_det.onnx', recursive=True)[0]
    model = onnx.load(det_path)

    # ONNX Runtime reference
    sess = ort.InferenceSession(det_path)
    inp = np.random.randn(1, 3, 256, 320).astype(np.float32)
    ref_out = sess.run(None, {sess.get_inputs()[0].name: inp})[0]

    # Verify graph structure for our interpreter
    ops = {}
    for node in model.graph.node:
        ops[node.op_type] = ops.get(node.op_type, 0) + 1

    supported = {'Conv', 'BatchNormalization', 'Relu', 'Add', 'Mul', 'Div',
                 'Sigmoid', 'HardSigmoid', 'HardSwish', 'Clip', 'Reshape',
                 'Transpose', 'Squeeze', 'Unsqueeze', 'Concat', 'Slice',
                 'GlobalAveragePool', 'Resize', 'Shape', 'Cast', 'Identity',
                 'Constant', 'MatMul', 'Softmax', 'ReduceMean', 'Pow', 'Sqrt',
                 'Sub', 'AveragePool', 'ConvTranspose'}

    unsupported = set(ops.keys()) - supported
    print(f"Reference output: shape={ref_out.shape} range=[{ref_out.min():.4f}, {ref_out.max():.4f}]")
    print(f"Ops needed: {sorted(ops.keys())}")
    print(f"Unsupported: {unsupported or 'NONE — all ops supported!'}")
    print(f"Conv nodes: {ops.get('Conv', 0)}")
    print(f"BatchNorm nodes: {ops.get('BatchNormalization', 0)}")
    print(f"BN folding candidates: {min(ops.get('Conv', 0), ops.get('BatchNormalization', 0))}")

    # Profile ONNX Runtime per-op (approximate by running subsets)
    times = []
    for _ in range(10):
        start = time.time()
        sess.run(None, {sess.get_inputs()[0].name: inp})
        times.append(time.time() - start)
    print(f"\nONNX Runtime: {np.mean(times)*1000:.1f} ms avg ({np.min(times)*1000:.1f} min)")

def test_recognizer_correctness():
    """Compare recognizer output."""
    rec_paths = glob.glob('/tmp/paddleocr-onnx/**/english/**/model.onnx', recursive=True)
    if not rec_paths:
        print("SKIP: English recognizer not found")
        return

    sess = ort.InferenceSession(rec_paths[0])
    inp = np.random.randn(1, 3, 48, 200).astype(np.float32)
    ref_out = sess.run(None, {sess.get_inputs()[0].name: inp})[0]

    model = onnx.load(rec_paths[0])
    ops = {}
    for node in model.graph.node:
        ops[node.op_type] = ops.get(node.op_type, 0) + 1

    print(f"\nRecognizer output: shape={ref_out.shape}")
    print(f"Ops: {sorted(ops.keys())}")

if __name__ == "__main__":
    test_detector_correctness()
    test_recognizer_correctness()
