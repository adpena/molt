"""Purpose: exceptions in custom exception handlers are trapped."""

import asyncio
import io
from contextlib import redirect_stderr


loop = asyncio.new_event_loop()
buf = io.StringIO()
try:
    asyncio.set_event_loop(loop)

    def handler(loop_arg, context):
        raise RuntimeError("handler boom")

    loop.set_exception_handler(handler)
    with redirect_stderr(buf):
        loop.call_exception_handler({"message": "outer"})
    text = buf.getvalue()
    print("raised", False)
    print("saw", "Unhandled error in exception handler" in text, "handler boom" in text)
finally:
    loop.close()
    asyncio.set_event_loop(None)
