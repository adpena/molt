from __future__ import annotations

import os
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
FALCON_OCR_ARTIFACT_ROOT = Path(
    os.environ.get(
        "MOLT_FALCON_OCR_ARTIFACT_ROOT",
        "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr",
    )
)
FALCON_OCR_WEIGHTS_DIR = FALCON_OCR_ARTIFACT_ROOT / "weights"
FALCON_OCR_MODEL_PATH = FALCON_OCR_WEIGHTS_DIR / "model.safetensors"
FALCON_OCR_CONFIG_PATH = FALCON_OCR_WEIGHTS_DIR / "config.json"
FALCON_OCR_TOKENIZER_PATH = FALCON_OCR_ARTIFACT_ROOT / "weights" / "tokenizer.json"


def falcon_ocr_weights_available() -> bool:
    return (
        FALCON_OCR_MODEL_PATH.is_file()
        and FALCON_OCR_CONFIG_PATH.is_file()
        and FALCON_OCR_TOKENIZER_PATH.is_file()
    )
