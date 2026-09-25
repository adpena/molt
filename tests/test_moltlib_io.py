from __future__ import annotations

import asyncio
import io
from pathlib import Path
from types import SimpleNamespace

import pytest

from moltlib import io as stream_io


@pytest.mark.parametrize("binary", [False, True])
def test_file_stream_chunks_and_eof(tmp_path: Path, binary: bool) -> None:
    path = tmp_path / "data"
    path.write_bytes(b"abcdefg")
    reader = stream_io.stream(path, "rb" if binary else "r", chunk_size=3)
    expected = [b"abc", b"def", b"g"] if binary else ["abc", "def", "g"]
    assert list(reader) == expected
    assert reader.closed
    assert list(reader) == []
    reader.close()
    with pytest.raises(ValueError, match="closed"):
        reader.__enter__()


def test_stream_async_context_closes_after_early_exit(tmp_path: Path) -> None:
    path = tmp_path / "data"
    path.write_bytes(b"abcdefg")

    async def consume() -> None:
        async with stream_io.stream(path, chunk_size=2) as reader:
            assert reader.__aiter__() is reader
            assert await anext(reader) == b"ab"
        assert reader.closed
        with pytest.raises(StopAsyncIteration):
            await anext(reader)
        await reader.aclose()

    asyncio.run(consume())


def test_stream_sync_context_closes_after_consumer_error(tmp_path: Path) -> None:
    path = tmp_path / "data"
    path.write_bytes(b"abc")
    reader = stream_io.stream(path, chunk_size=2)
    with pytest.raises(LookupError):
        with reader:
            assert next(reader) == b"ab"
            raise LookupError("consumer")
    assert reader.closed


@pytest.mark.parametrize("size", [0, -1, 1.5, "3"])
def test_invalid_chunk_size_never_opens_or_truncates(
    tmp_path: Path, size: object
) -> None:
    path = tmp_path / "data"
    path.write_bytes(b"preserve")
    with pytest.raises((TypeError, ValueError)):
        stream_io.stream(path, "wb", chunk_size=size)  # type: ignore[arg-type]
    assert path.read_bytes() == b"preserve"


def test_index_chunk_size_and_open_keywords(tmp_path: Path) -> None:
    class ChunkSize:
        def __index__(self) -> int:
            return 2

    path = tmp_path / "data"
    path.write_bytes(b"a\r\nb\r\n")
    with stream_io.stream(
        path,
        "r",
        chunk_size=ChunkSize(),  # type: ignore[arg-type]
        encoding="ascii",
        newline="",
    ) as reader:
        assert "".join(reader) == "a\r\nb\r\n"


def test_public_io_owns_open_refusal(monkeypatch: pytest.MonkeyPatch) -> None:
    refusal = PermissionError("fs.read")

    def denied(*args, **kwargs):
        raise refusal

    monkeypatch.setattr(stream_io, "io", SimpleNamespace(open=denied))
    with pytest.raises(PermissionError) as result:
        stream_io.stream("denied")
    assert result.value is refusal


@pytest.mark.parametrize("close_fails", [False, True])
def test_read_failure_closes_once_and_preserves_exception_chain(
    monkeypatch: pytest.MonkeyPatch, close_fails: bool
) -> None:
    read_error = OSError("read")
    close_error = OSError("close")

    class BrokenFile:
        close_count = 0

        def read(self, size: int) -> bytes:
            assert size == 3
            raise read_error

        def close(self) -> None:
            self.close_count += 1
            if close_fails:
                raise close_error

    handle = BrokenFile()
    monkeypatch.setattr(stream_io, "io", SimpleNamespace(open=lambda *a, **k: handle))
    reader = stream_io.stream("data", chunk_size=3)
    with pytest.raises(OSError) as result:
        next(reader)
    assert result.value is (close_error if close_fails else read_error)
    if close_fails:
        assert result.value.__context__ is read_error
    reader.close()
    assert reader.closed
    assert handle.close_count == 1


def test_async_read_is_pull_driven(monkeypatch: pytest.MonkeyPatch) -> None:
    class CountingFile(io.BytesIO):
        reads = 0

        def read(self, size: int = -1) -> bytes:
            self.reads += 1
            return super().read(size)

    handle = CountingFile(b"abc")
    monkeypatch.setattr(stream_io, "io", SimpleNamespace(open=lambda *a, **k: handle))
    reader = stream_io.stream("data", chunk_size=2)
    assert handle.reads == 0

    async def consume() -> None:
        assert await anext(reader) == b"ab"
        assert handle.reads == 1
        assert await anext(reader) == b"c"
        assert handle.reads == 2
        with pytest.raises(StopAsyncIteration):
            await anext(reader)
        assert handle.reads == 3
        assert handle.closed

    asyncio.run(consume())
