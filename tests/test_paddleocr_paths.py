from __future__ import annotations

from pathlib import Path

import pytest

from tests.helpers.paddleocr_paths import (
    PADDLEOCR_MODEL_ROOT_ENV,
    hf_snapshot_artifact_candidate,
    paddleocr_model_root,
    require_paddleocr_artifact,
)


def test_paddleocr_paths_use_env_root(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    artifact = tmp_path / "ch_PP-OCRv4_det.onnx"
    artifact.write_bytes(b"onnx")
    monkeypatch.setenv(PADDLEOCR_MODEL_ROOT_ENV, str(tmp_path))

    assert paddleocr_model_root() == tmp_path
    assert require_paddleocr_artifact("ch_PP-OCRv4_det.onnx") == artifact


def test_paddleocr_paths_skip_when_root_missing(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    missing_root = tmp_path / "missing"
    monkeypatch.setenv(PADDLEOCR_MODEL_ROOT_ENV, str(missing_root))

    with pytest.raises(pytest.skip.Exception, match="is not a directory"):
        require_paddleocr_artifact("ch_PP-OCRv4_det.onnx")


def test_paddleocr_paths_skip_when_artifact_missing(tmp_path: Path) -> None:
    with pytest.raises(pytest.skip.Exception, match="Missing PaddleOCR artifact"):
        require_paddleocr_artifact("ch_PP-OCRv4_det.onnx", root=tmp_path)


def test_paddleocr_hf_snapshot_candidate_uses_refs_main(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    refs = tmp_path / "models--demo--paddleocr" / "refs"
    refs.mkdir(parents=True)
    (refs / "main").write_text("abc123\n", encoding="utf-8")
    monkeypatch.setenv(PADDLEOCR_MODEL_ROOT_ENV, str(tmp_path))

    assert (
        hf_snapshot_artifact_candidate(
            "models--demo--paddleocr", "rec/english/model.onnx"
        )
        == "models--demo--paddleocr/snapshots/abc123/rec/english/model.onnx"
    )
