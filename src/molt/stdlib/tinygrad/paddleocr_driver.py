"""
WASM/native entry point for PaddleOCR inference via molt/tinygrad.

This module provides a minimal API surface for compiled PaddleOCR:
  - init(): load ONNX model weights
  - ocr():  run full detect -> classify -> recognize pipeline

Compiled with:
    molt build paddleocr_driver.py --target wasm

Exports:
    init(detector_bytes: bytes, recognizer_bytes: bytes,
         dict_bytes: bytes) -> None
    ocr(width: int, height: int, rgb: bytes) -> str
    detect_only(width: int, height: int, rgb: bytes) -> str

The ONNX model weights are loaded once at init time.  Each ocr() call
runs the full PaddleOCR pipeline:

  1. Preprocess: normalize RGB pixels to [0,1], apply ImageNet mean/std
  2. Detect:     DBNet text detector -> bounding boxes
  3. Classify:   Direction classifier -> 0/180 degree rotation
  4. Recognize:  SVTRv2 + CTC decode -> text strings

Total model size: ~16.1 MB (detector 4.7 + classifier 0.6 + recognizer 10.8)
"""

from __future__ import annotations
from _intrinsics import require_intrinsic as _require_intrinsic

_gpu_device = _require_intrinsic("molt_gpu_prim_device")

from tinygrad.tensor import Tensor
from tinygrad.dtypes import dtypes
from tinygrad.paddleocr import PaddleOCR
from tinygrad.lazy import LazyOp, LazyBuffer


_ocr: PaddleOCR | None = None


def init(detector_bytes: bytes, recognizer_bytes: bytes, dict_bytes: bytes) -> None:
    """Initialize PaddleOCR with ONNX model weights.

    Args:
        detector_bytes:   Raw bytes of ch_PP-OCRv4_det.onnx (4.7 MB).
        recognizer_bytes: Raw bytes of ch_PP-OCRv4_rec.onnx (10.8 MB).
        dict_bytes:       Raw bytes of the character dictionary file.
                         One character per line, 6623 entries for PP-OCRv4.

    The classifier is optional for the initial release — text regions are
    assumed to be correctly oriented (0 degrees).
    """
    global _ocr
    _ocr = PaddleOCR()
    _ocr.load_detector(detector_bytes)
    _ocr.load_recognizer(recognizer_bytes, dict_bytes.decode("utf-8"))


def init_full(
    detector_bytes: bytes,
    classifier_bytes: bytes,
    recognizer_bytes: bytes,
    dict_bytes: bytes,
) -> None:
    """Initialize with all three models including direction classifier.

    Args:
        detector_bytes:   Raw bytes of ch_PP-OCRv4_det.onnx (4.7 MB).
        classifier_bytes: Raw bytes of ch_ppocr_mobile_v2.0_cls.onnx (0.6 MB).
        recognizer_bytes: Raw bytes of ch_PP-OCRv4_rec.onnx (10.8 MB).
        dict_bytes:       Character dictionary bytes (UTF-8, one char/line).
    """
    global _ocr
    _ocr = PaddleOCR()
    _ocr.load_detector(detector_bytes)
    _ocr.load_classifier(classifier_bytes)
    _ocr.load_recognizer(recognizer_bytes, dict_bytes.decode("utf-8"))


def ocr(width: int, height: int, rgb: bytes) -> str:
    """Run full OCR pipeline on an RGB image.

    Args:
        width:  Image width in pixels.
        height: Image height in pixels.
        rgb:    Raw RGB pixel bytes (width * height * 3 bytes, row-major).

    Returns:
        Newline-separated recognized text lines, each formatted as:
          "confidence|x1,y1,x2,y2|text"
        Example: "0.97|10,20,300,45|Invoice #42"
    """
    if _ocr is None:
        raise RuntimeError("Call init() first")

    image_tensor = _rgb_bytes_to_tensor(width, height, rgb)
    results = _ocr.recognize(image_tensor)
    lines: list[str] = []
    for r in results:
        conf = r.get("confidence", 0.0)
        text = r.get("text", "")
        bbox = r.get("bbox", (0, 0, 0, 0))
        lines.append(f"{conf:.2f}|{bbox[0]},{bbox[1]},{bbox[2]},{bbox[3]}|{text}")
    return "\n".join(lines)


def detect_only(width: int, height: int, rgb: bytes) -> str:
    """Run text detection only (no recognition).

    Returns:
        Newline-separated bounding boxes: "x1,y1,x2,y2" per line.
    """
    if _ocr is None:
        raise RuntimeError("Call init() first")

    image_tensor = _rgb_bytes_to_tensor(width, height, rgb)
    preprocessed = _ocr.preprocess(image_tensor)
    boxes = _ocr.detector.detect(preprocessed)
    return "\n".join(f"{x1},{y1},{x2},{y2}" for x1, y1, x2, y2 in boxes)


def _rgb_bytes_to_tensor(width: int, height: int, rgb: bytes) -> Tensor:
    """Convert raw RGB bytes to [1, 3, H, W] float32 tensor.

    Input: row-major RGB bytes (R,G,B,R,G,B,...) with values 0-255.
    Output: [1, 3, H, W] tensor with pixel values 0.0-255.0.
    """
    n_pixels = width * height
    if len(rgb) != n_pixels * 3:
        raise ValueError(
            f"Expected {n_pixels * 3} bytes for {width}x{height} RGB, got {len(rgb)}"
        )

    # Deinterleave RGB -> separate R, G, B channels
    r_data = [0.0] * n_pixels
    g_data = [0.0] * n_pixels
    b_data = [0.0] * n_pixels

    for i in range(n_pixels):
        r_data[i] = float(rgb[i * 3])
        g_data[i] = float(rgb[i * 3 + 1])
        b_data[i] = float(rgb[i * 3 + 2])

    # Layout: [1, 3, H, W] = [batch, channel, row, col]
    data = r_data + g_data + b_data
    shape = (1, 3, height, width)
    op = LazyOp("LOAD", (), dtype=dtypes.float32, shape=shape)
    return Tensor(LazyBuffer(op, dtypes.float32, shape, data=data))
