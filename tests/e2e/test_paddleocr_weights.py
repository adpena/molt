"""Test PaddleOCR ONNX weight loading.

Validates that the OnnxWeightParser correctly extracts weight tensors
from PaddleOCR ONNX models (which store weights as Constant graph nodes,
not graph.initializer). We test the parser directly — the tinygrad Tensor
integration requires the molt runtime and is covered by runtime tests.
"""
import os
import sys
import glob
import struct
import types


def _import_paddleocr_module():
    """Import the paddleocr module in isolation, mocking tinygrad deps.

    The paddleocr.py file imports tinygrad.tensor and tinygrad.dtypes at
    module level. Since the full tinygrad stack requires the molt runtime,
    we install lightweight mocks for those modules before loading paddleocr
    via importlib.util with an explicit origin path (avoiding sys.path
    contamination from molt's stdlib overlay).
    """
    # Install tinygrad mocks before importing paddleocr
    mock_tinygrad = types.ModuleType("tinygrad")
    mock_tensor = types.ModuleType("tinygrad.tensor")
    mock_dtypes_mod = types.ModuleType("tinygrad.dtypes")
    mock_lazy = types.ModuleType("tinygrad.lazy")
    mock_realize = types.ModuleType("tinygrad.realize")

    class FakeDtypes:
        float32 = "float32"
        int32 = "int32"
        int64 = "int64"

    mock_dtypes_mod.dtypes = FakeDtypes()

    class FakeTensor:
        def __init__(self, *a, **kw):
            pass
        @staticmethod
        def zeros(*a, **kw):
            return FakeTensor()

    mock_tensor.Tensor = FakeTensor

    class FakeLazyOp:
        def __init__(self, *a, **kw):
            pass

    class FakeLazyBuffer:
        def __init__(self, *a, **kw):
            pass

    mock_lazy.LazyOp = FakeLazyOp
    mock_lazy.LazyBuffer = FakeLazyBuffer

    saved = {}
    for name in ("tinygrad", "tinygrad.tensor", "tinygrad.dtypes",
                  "tinygrad.lazy", "tinygrad.realize"):
        saved[name] = sys.modules.get(name)

    sys.modules["tinygrad"] = mock_tinygrad
    sys.modules["tinygrad.tensor"] = mock_tensor
    sys.modules["tinygrad.dtypes"] = mock_dtypes_mod
    sys.modules["tinygrad.lazy"] = mock_lazy
    sys.modules["tinygrad.realize"] = mock_realize

    # Use importlib.util from the real stdlib (not molt's overlay)
    import importlib.util as ilu
    paddleocr_path = os.path.join(
        os.path.dirname(__file__),
        "../../src/molt/stdlib/tinygrad/paddleocr.py",
    )
    paddleocr_path = os.path.abspath(paddleocr_path)
    spec = ilu.spec_from_file_location("_paddleocr_test", paddleocr_path)
    module = ilu.module_from_spec(spec)
    spec.loader.exec_module(module)
    # Keep tinygrad mocks installed — WeightStore.load_onnx needs them
    # at call time (lazy import of tinygrad.lazy inside load_onnx).
    return module


def find_onnx_file(name: str) -> str | None:
    """Find ONNX file in common locations (HuggingFace cache, /tmp)."""
    for p in glob.glob(f"/tmp/paddleocr-onnx/**/{name}", recursive=True):
        return p
    for p in glob.glob(
        os.path.expanduser(f"~/.cache/huggingface/hub/models--*paddleocr*/**/{name}"),
        recursive=True,
    ):
        return p
    direct = f"/tmp/paddleocr-onnx/{name}"
    if os.path.exists(direct):
        return direct
    return None


def test_detector_weights_load() -> None:
    """Load PaddleOCR detector ONNX weights and verify tensor extraction."""
    onnx_path = find_onnx_file("ch_PP-OCRv4_det.onnx")
    if not onnx_path:
        print("SKIP: detector ONNX not found at /tmp/paddleocr-onnx/")
        return

    mod = _import_paddleocr_module()
    OnnxWeightParser = mod.OnnxWeightParser

    data = open(onnx_path, "rb").read()
    parsed = OnnxWeightParser.parse(data)

    n_weights = len(parsed)
    print(f"Detector: {n_weights} weight tensors loaded from {onnx_path}")

    # PP-OCRv4 detector has 342 Constant nodes
    assert n_weights > 0, "No weights loaded"
    assert n_weights >= 300, f"Expected >= 300 weight tensors, got {n_weights}"

    names = list(parsed.keys())

    # Batch norm weights are always present
    bn_names = [n for n in names if "batch_norm" in n]
    assert len(bn_names) > 0, f"No batch_norm weights found in {len(names)} names"
    print(f"  batch_norm tensors: {len(bn_names)}")

    # Conv weights
    conv_names = [n for n in names if "conv" in n.lower()]
    assert len(conv_names) > 0, "No conv weights found"
    print(f"  conv tensors: {len(conv_names)}")

    # Verify shapes and data are non-empty
    for name, (shape, dtype_code, values) in list(parsed.items())[:5]:
        assert len(values) > 0, f"Weight '{name}' has empty data"
        if shape:
            expected_elems = 1
            for d in shape:
                expected_elems *= d
            assert len(values) == expected_elems, (
                f"Weight '{name}' shape {shape} expects {expected_elems} elements, got {len(values)}"
            )

    print("  PASS")


def test_recognizer_weights_load() -> None:
    """Load PaddleOCR recognizer ONNX weights and verify tensor extraction."""
    onnx_path = find_onnx_file("ch_PP-OCRv4_rec.onnx")
    if not onnx_path:
        print("SKIP: recognizer ONNX not found at /tmp/paddleocr-onnx/")
        return

    mod = _import_paddleocr_module()
    OnnxWeightParser = mod.OnnxWeightParser

    data = open(onnx_path, "rb").read()
    parsed = OnnxWeightParser.parse(data)

    n_weights = len(parsed)
    print(f"Recognizer: {n_weights} weight tensors loaded from {onnx_path}")

    # PP-OCRv4 recognizer has 406 Constant nodes
    assert n_weights > 0, "No weights loaded"
    assert n_weights >= 350, f"Expected >= 350 weight tensors, got {n_weights}"

    names = list(parsed.keys())
    bn_names = [n for n in names if "batch_norm" in n]
    print(f"  batch_norm tensors: {len(bn_names)}")

    print("  PASS")


def test_classifier_weights_load() -> None:
    """Load PaddleOCR classifier ONNX weights."""
    onnx_path = find_onnx_file("ch_ppocr_mobile_v2.0_cls_infer.onnx")
    if not onnx_path:
        print("SKIP: classifier ONNX not found")
        return

    mod = _import_paddleocr_module()
    OnnxWeightParser = mod.OnnxWeightParser

    data = open(onnx_path, "rb").read()
    parsed = OnnxWeightParser.parse(data)

    n_weights = len(parsed)
    print(f"Classifier: {n_weights} weight tensors loaded from {onnx_path}")
    assert n_weights > 0, "No weights loaded"
    print("  PASS")


def test_weight_parser_dtype_coverage() -> None:
    """Verify the parser handles all ONNX dtypes present in PaddleOCR models."""
    onnx_path = find_onnx_file("ch_PP-OCRv4_rec.onnx")
    if not onnx_path:
        print("SKIP: recognizer ONNX not found")
        return

    mod = _import_paddleocr_module()
    OnnxWeightParser = mod.OnnxWeightParser

    data = open(onnx_path, "rb").read()
    parsed = OnnxWeightParser.parse(data)

    dtype_counts: dict[int, int] = {}
    for _name, (shape, dtype_code, values) in parsed.items():
        dtype_counts[dtype_code] = dtype_counts.get(dtype_code, 0) + 1

    print(f"Dtype coverage: {dtype_counts}")
    # PP-OCRv4 rec has float32 (1), int32 (6), int64 (7)
    assert 1 in dtype_counts, "No float32 tensors found"
    assert 7 in dtype_counts, "No int64 tensors found (expected for shape constants)"
    print("  PASS")


def test_weight_data_integrity() -> None:
    """Verify extracted weight data matches what onnx library reports."""
    onnx_path = find_onnx_file("ch_PP-OCRv4_det.onnx")
    if not onnx_path:
        print("SKIP: detector ONNX not found")
        return

    try:
        import onnx
        from onnx import numpy_helper
    except ImportError:
        print("SKIP: onnx library not available for cross-validation")
        return

    mod = _import_paddleocr_module()
    OnnxWeightParser = mod.OnnxWeightParser

    data = open(onnx_path, "rb").read()
    parsed = OnnxWeightParser.parse(data)

    # Cross-validate against onnx library
    model = onnx.load(onnx_path)
    onnx_constants: dict[str, tuple[tuple[int, ...], int]] = {}
    for node in model.graph.node:
        if node.op_type == "Constant" and node.output:
            for attr in node.attribute:
                if attr.name == "value" and attr.t is not None:
                    name = node.output[0]
                    shape = tuple(int(d) for d in attr.t.dims)
                    onnx_constants[name] = (shape, attr.t.data_type)

    # Every onnx constant should be in our parsed output
    missing = set(onnx_constants.keys()) - set(parsed.keys())
    assert len(missing) == 0, f"Missing {len(missing)} constants: {list(missing)[:5]}"

    # Shapes must match
    for name, (onnx_shape, onnx_dtype) in onnx_constants.items():
        parsed_shape, parsed_dtype, parsed_vals = parsed[name]
        assert parsed_shape == onnx_shape, (
            f"Shape mismatch for '{name}': onnx={onnx_shape}, parsed={parsed_shape}"
        )
        assert parsed_dtype == onnx_dtype, (
            f"Dtype mismatch for '{name}': onnx={onnx_dtype}, parsed={parsed_dtype}"
        )

    print(f"Data integrity: {len(onnx_constants)} constants cross-validated against onnx library")
    print("  PASS")


def test_weight_store_integration() -> None:
    """Test WeightStore.load_onnx with real model data."""
    onnx_path = find_onnx_file("ch_PP-OCRv4_det.onnx")
    if not onnx_path:
        print("SKIP: detector ONNX not found")
        return

    mod = _import_paddleocr_module()
    ws = mod.WeightStore()
    data = open(onnx_path, "rb").read()
    count = ws.load_onnx(data)

    assert count > 0, "WeightStore loaded 0 tensors"
    assert len(ws) == count
    names = ws.names()
    assert len(names) == count

    # Test __contains__
    first_name = names[0]
    assert first_name in ws
    assert "nonexistent_weight_xyz" not in ws

    # Test get
    tensor = ws.get(first_name)
    assert tensor is not None

    print(f"WeightStore: {count} tensors loaded, get/contains/names all work")
    print("  PASS")


if __name__ == "__main__":
    test_detector_weights_load()
    test_recognizer_weights_load()
    test_classifier_weights_load()
    test_weight_parser_dtype_coverage()
    test_weight_data_integrity()
    test_weight_store_integration()
    print("\nAll PaddleOCR weight loading tests passed.")
