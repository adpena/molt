from __future__ import annotations

import os
from pathlib import Path

import pytest


PADDLEOCR_MODEL_ROOT_ENV = "MOLT_PADDLEOCR_MODEL_ROOT"
DEFAULT_PADDLEOCR_MODEL_ROOT = Path("/tmp/paddleocr-onnx")


def paddleocr_model_root() -> Path:
    raw = os.environ.get(PADDLEOCR_MODEL_ROOT_ENV)
    return Path(raw).expanduser() if raw else DEFAULT_PADDLEOCR_MODEL_ROOT


def hf_snapshot_artifact_candidate(repo_cache_name: str, artifact_relative: str) -> str:
    """Return the deterministic HuggingFace-cache relative path for an artifact.

    The snapshot id comes from ``refs/main`` under the configured model root.
    This avoids recursive globbing while still supporting the standard
    HuggingFace cache layout when the root points at its cache directory.
    """
    root = paddleocr_model_root()
    ref_path = root / repo_cache_name / "refs" / "main"
    if not ref_path.is_file():
        return f"{repo_cache_name}/snapshots/__missing_ref__/{artifact_relative}"
    snapshot = ref_path.read_text(encoding="utf-8").strip()
    return f"{repo_cache_name}/snapshots/{snapshot}/{artifact_relative}"


def require_paddleocr_artifact(
    *relative_candidates: str,
    root: Path | None = None,
) -> Path:
    model_root = (root or paddleocr_model_root()).expanduser()
    if not model_root.is_dir():
        pytest.skip(f"{PADDLEOCR_MODEL_ROOT_ENV} is not a directory: {model_root}")
    for relative in relative_candidates:
        candidate = model_root / relative
        if candidate.is_file():
            return candidate
    tried = ", ".join(str(model_root / relative) for relative in relative_candidates)
    pytest.skip(f"Missing PaddleOCR artifact under {model_root}; tried: {tried}")
