import asyncio
import builtins
import io as py_io
import os
import sys
import types

import pytest

from molt import intrinsics as _intrinsics


registry = getattr(builtins, "_molt_intrinsics", None)
if not isinstance(registry, dict):
    registry = {}
    setattr(builtins, "_molt_intrinsics", registry)
registry.setdefault("molt_stdlib_probe", lambda: True)
registry.setdefault("molt_capabilities_has", lambda _name=None: True)


def _capabilities_require(name: str) -> None:
    raw = os.environ.get("MOLT_CAPABILITIES", "")
    granted = {item.strip() for item in raw.split(",") if item.strip()}
    if name not in granted:
        raise PermissionError(name)


registry.setdefault("molt_capabilities_require", _capabilities_require)
registry.setdefault("molt_file_open_ex", lambda file, mode, *_args: open(file, mode))
registry.setdefault("molt_file_read", lambda handle, size=None: handle.read(size))
registry.setdefault("molt_file_close", lambda handle: handle.close())
registry.setdefault("molt_io_class", lambda name: getattr(py_io, name))


class _TestStream:
    def __init__(self, source) -> None:
        self._source = source

    async def _iterate(self):
        for chunk in self._source:
            yield chunk

    def __aiter__(self):
        return self._iterate()


fake_net = types.ModuleType("molt.net")
fake_net.Stream = _TestStream
sys.modules["molt.net"] = fake_net


if not _intrinsics.runtime_active():
    pytest.skip("Molt runtime intrinsics not active", allow_module_level=True)

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
