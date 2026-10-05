"""Purpose: differential coverage for asyncio.default_exception_handler."""

import asyncio
import contextlib
import io


class RecordingStream:
    def __init__(self):
        self.text = ""
        self.flushes = 0

    def write(self, text):
        self.text += text
        return len(text)

    def flush(self):
        self.flushes += 1


loop = asyncio.new_event_loop()
try:
    buf = io.StringIO()
    with contextlib.redirect_stderr(buf):
        loop.default_exception_handler(
            {"message": "probe", "exception": RuntimeError("x")}
        )
    captured = buf.getvalue()
    print("probe" in captured, "x" in captured)
    # Reporting follows the current stream on each invocation and keeps the
    # Python method protocol. A cached native stderr handle cannot substitute.
    first = RecordingStream()
    second = RecordingStream()
    with contextlib.redirect_stderr(first):
        loop.default_exception_handler({"message": "first-stream"})
    with contextlib.redirect_stderr(second):
        loop.default_exception_handler({"message": "second-stream"})
    print("streams", "first-stream" in first.text, "second-stream" in second.text)
    print(
        "separate", "second-stream" not in first.text, "first-stream" not in second.text
    )
    print("flushed", first.flushes > 0, second.flushes > 0)
finally:
    loop.close()
