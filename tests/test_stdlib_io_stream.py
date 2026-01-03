import asyncio

import pytest

from molt.stdlib import io


def test_stream_requires_capability(tmp_path, monkeypatch):
    path = tmp_path / "data.bin"
    path.write_bytes(b"hello")
    monkeypatch.setenv("MOLT_CAPABILITIES", "")
    with pytest.raises(PermissionError):
        io.stream(path)


def test_stream_backpressure(tmp_path, monkeypatch):
    data = b"hello world"
    path = tmp_path / "data.bin"
    path.write_bytes(data)
    monkeypatch.setenv("MOLT_CAPABILITIES", "fs.read")

    stream = io.stream(path, chunk_size=3)

    async def read_all():
        out = bytearray()
        async for chunk in stream:
            out.extend(chunk)
            await asyncio.sleep(0)
        return bytes(out)

    assert asyncio.run(read_all()) == data
