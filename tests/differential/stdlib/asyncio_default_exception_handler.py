"""Purpose: differential coverage for asyncio.default_exception_handler."""

import asyncio
import contextlib
import io


loop = asyncio.new_event_loop()
try:
    buf = io.StringIO()
    with contextlib.redirect_stderr(buf):
        loop.default_exception_handler(
            {"message": "probe", "exception": RuntimeError("x")}
        )
    captured = buf.getvalue()
    print("probe" in captured, "x" in captured)
finally:
    loop.close()
