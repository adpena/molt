from __future__ import annotations

from io import BytesIO

from molt_accel.framing import read_frame, write_frame


def test_frame_roundtrip() -> None:
    buf = BytesIO()
    write_frame(buf, b"hello")
    buf.seek(0)
    assert read_frame(buf) == b"hello"
