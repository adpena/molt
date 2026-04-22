from __future__ import annotations

import struct
import urllib.error
import urllib.request
from pathlib import Path

import pytest

from molt.gpu.gguf import GGUF_MAGIC, GGUF_TYPE_Q5_0, load_gguf
from molt.gpu.hub import download_model


def _write_gguf_string(handle, value: str) -> None:
    encoded = value.encode("utf-8")
    handle.write(struct.pack("<Q", len(encoded)))
    handle.write(encoded)


def _write_gguf_fixture(
    path: Path,
    *,
    metadata: list[tuple[str, int, bytes]] = (),
    tensors: list[tuple[str, int, list[int], int, bytes]] = (),
) -> None:
    with path.open("wb") as handle:
        handle.write(struct.pack("<I", GGUF_MAGIC))
        handle.write(struct.pack("<I", 3))
        handle.write(struct.pack("<Q", len(tensors)))
        handle.write(struct.pack("<Q", len(metadata)))

        for key, vtype, payload in metadata:
            _write_gguf_string(handle, key)
            handle.write(struct.pack("<I", vtype))
            handle.write(payload)

        for name, ndim, dims, dtype, payload in tensors:
            _write_gguf_string(handle, name)
            handle.write(struct.pack("<I", ndim))
            for dim in dims:
                handle.write(struct.pack("<Q", dim))
            handle.write(struct.pack("<I", dtype))
            handle.write(struct.pack("<Q", 0))

        padding = (32 - (handle.tell() % 32)) % 32
        handle.write(b"\0" * padding)
        for _, _, _, _, payload in tensors:
            handle.write(payload)


def test_load_gguf_rejects_unsupported_metadata_type(tmp_path: Path) -> None:
    path = tmp_path / "bad-metadata.gguf"
    _write_gguf_fixture(
        path,
        metadata=[
            ("general.architecture", 99, b""),
        ],
    )

    with pytest.raises(ValueError, match="Unsupported GGUF metadata type"):
        load_gguf(str(path))


def test_load_gguf_rejects_q5_tensor_type(tmp_path: Path) -> None:
    path = tmp_path / "q5.gguf"
    _write_gguf_fixture(
        path,
        metadata=[
            ("general.architecture", 8, struct.pack("<Q", 5) + b"llama"),
        ],
        tensors=[
            ("blk.0.weight", 1, [1], GGUF_TYPE_Q5_0, b"\0" * 4),
        ],
    )

    with pytest.raises(ValueError, match="Unsupported GGUF tensor type"):
        load_gguf(str(path))


def test_download_model_without_filename_does_not_guess_gguf(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[str] = []

    class _Response:
        def __init__(self, payload: bytes):
            self._payload = payload
            self.headers = {"Content-Length": str(len(payload))}

        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb):
            return False

        def read(self, size: int = -1) -> bytes:
            if size < 0 or size >= len(self._payload):
                data, self._payload = self._payload, b""
                return data
            data, self._payload = self._payload[:size], self._payload[size:]
            return data

    def fake_urlopen(request):
        url = getattr(request, "full_url", request)
        calls.append(url)
        if url.endswith("model.gguf"):
            return _Response(b"gguf")
        raise urllib.error.HTTPError(url, 404, "Not Found", hdrs=None, fp=None)

    monkeypatch.setattr(urllib.request, "urlopen", fake_urlopen)

    with pytest.raises(FileNotFoundError, match="No model file found"):
        download_model("demo/model", cache_dir=str(tmp_path))

    assert all(not call.endswith("model.gguf") for call in calls)
