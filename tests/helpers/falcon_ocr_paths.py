"""Where the Falcon OCR experiment's external artifacts live.

The weights, the tokenizer and the helper modules come from an external
experiment checkout, not from a build, so they have no default location: a
test that needs them skips unless MOLT_FALCON_OCR_ARTIFACT_ROOT names that
checkout.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

ARTIFACT_ROOT_ENV = "MOLT_FALCON_OCR_ARTIFACT_ROOT"
_WEIGHT_FILES = ("model.safetensors", "config.json", "tokenizer.json")


def falcon_ocr_artifact_root() -> Path | None:
    raw = os.environ.get(ARTIFACT_ROOT_ENV, "").strip()
    return Path(raw).expanduser() if raw else None


def falcon_ocr_weight_path(name: str) -> Path | None:
    root = falcon_ocr_artifact_root()
    return None if root is None else root / "weights" / name


def falcon_ocr_weights_available() -> bool:
    paths = [falcon_ocr_weight_path(name) for name in _WEIGHT_FILES]
    return all(path is not None and path.is_file() for path in paths)


def require_falcon_ocr_artifact_root() -> Path:
    root = falcon_ocr_artifact_root()
    if root is None or not root.is_dir():
        pytest.skip(f"{ARTIFACT_ROOT_ENV} does not name the Falcon OCR checkout")
    return root


def require_falcon_ocr_weight(name: str) -> Path:
    path = falcon_ocr_weight_path(name)
    if path is None or not path.is_file():
        pytest.skip(f"Falcon OCR {name} is not under {ARTIFACT_ROOT_ENV}/weights")
    return path
