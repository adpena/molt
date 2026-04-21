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
FALCON_OCR_TOKENIZER_PATH = FALCON_OCR_ARTIFACT_ROOT / "weights" / "tokenizer.json"
