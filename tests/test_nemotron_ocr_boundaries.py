"""Boundary tests for the Nemotron OCR deployment wrapper."""

from __future__ import annotations

import asyncio
import base64
import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
NEMOTRON_MODAL_PATH = REPO_ROOT / "deploy" / "modal" / "nemotron_ocr.py"


def _identity_decorator(*args, **kwargs):
    if len(args) == 1 and callable(args[0]) and not kwargs:
        return args[0]

    def decorate(obj):
        return obj

    return decorate


class _FakeModalImage:
    @classmethod
    def from_registry(cls, *args, **kwargs):
        return cls()

    def pip_install(self, *args, **kwargs):
        return self

    def run_commands(self, *args, **kwargs):
        return self


class _FakeModalApp:
    def __init__(self, *args, **kwargs):
        pass

    def cls(self, *args, **kwargs):
        return _identity_decorator

    def function(self, *args, **kwargs):
        return _identity_decorator

    def local_entrypoint(self, *args, **kwargs):
        return _identity_decorator


def _load_nemotron_modal_module(monkeypatch: pytest.MonkeyPatch):
    fake_modal = SimpleNamespace(
        App=_FakeModalApp,
        Image=_FakeModalImage,
        enter=_identity_decorator,
        method=_identity_decorator,
        web_endpoint=_identity_decorator,
    )
    monkeypatch.setitem(sys.modules, "modal", fake_modal)
    module_name = "test_loaded_nemotron_ocr"
    sys.modules.pop(module_name, None)
    spec = importlib.util.spec_from_file_location(module_name, NEMOTRON_MODAL_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def _install_fake_pillow(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakeImage:
        def convert(self, mode: str):
            assert mode == "RGB"
            return self

        def save(self, target, format: str) -> None:
            assert format == "PNG"
            target.write(b"fake-png")

    def fake_open(stream):
        assert stream.read() == b"raw-image"
        return FakeImage()

    image_module = SimpleNamespace(open=fake_open)
    monkeypatch.setitem(sys.modules, "PIL", SimpleNamespace(Image=image_module))
    monkeypatch.setitem(sys.modules, "PIL.Image", image_module)


def _image_b64() -> str:
    return base64.b64encode(b"raw-image").decode("ascii")


def test_nemotron_modal_rejects_unsupported_lang_and_merge_level(monkeypatch):
    module = _load_nemotron_modal_module(monkeypatch)

    assert module._normalize_lang("en") == "en"
    assert module._normalize_lang("english") == "en"
    assert module._normalize_lang("multi") == "multi"
    assert module._normalize_lang("multilingual") == "multi"

    with pytest.raises(ValueError, match="Unsupported lang"):
        module._normalize_lang("fr")

    assert module._normalize_merge_level("word") == "word"
    assert module._normalize_merge_level("sentence") == "sentence"
    assert module._normalize_merge_level("paragraph") == "paragraph"

    with pytest.raises(ValueError, match="Unsupported merge_level"):
        module._normalize_merge_level("page")


def test_nemotron_modal_temp_png_context_removes_file(monkeypatch):
    module = _load_nemotron_modal_module(monkeypatch)
    _install_fake_pillow(monkeypatch)

    with module._temporary_rgb_png_from_base64(_image_b64()) as path:
        temp_path = Path(path)
        assert temp_path.exists()
        assert temp_path.read_bytes() == b"fake-png"

    assert not temp_path.exists()


def test_nemotron_modal_batch_uses_single_pipeline_call_and_cleans_temp_files(
    monkeypatch,
):
    module = _load_nemotron_modal_module(monkeypatch)
    _install_fake_pillow(monkeypatch)
    calls: list[tuple[list[str], str]] = []

    class FakeModel:
        def __call__(self, image_paths, merge_level: str):
            paths = list(image_paths)
            calls.append((paths, merge_level))
            assert all(Path(path).exists() for path in paths)
            return [
                [
                    {
                        "text": "first",
                        "confidence": 0.98765,
                        "left": 1.0,
                        "upper": 2.0,
                        "right": 3.0,
                        "lower": 4.0,
                    }
                ],
                [
                    {
                        "text": "second",
                        "confidence": 0.9,
                        "left": 5.0,
                        "upper": 6.0,
                        "right": 7.0,
                        "lower": 8.0,
                    }
                ],
            ]

    service = object.__new__(module.NemotronOCR)
    service.ocr_en = FakeModel()
    service.ocr_multi = FakeModel()

    results = asyncio.run(
        service.ocr_batch(
            [_image_b64(), _image_b64()],
            lang="english",
            merge_level="sentence",
        )
    )

    assert len(calls) == 1
    image_paths, merge_level = calls[0]
    assert merge_level == "sentence"
    assert all(not Path(path).exists() for path in image_paths)
    assert [result["full_text"] for result in results] == ["first", "second"]
    assert results[0]["regions"][0] == {
        "text": "first",
        "confidence": 0.9877,
        "bbox": [1.0, 2.0, 3.0, 2.0, 3.0, 4.0, 1.0, 4.0],
    }
