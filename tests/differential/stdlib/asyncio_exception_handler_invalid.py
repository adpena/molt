"""Purpose: set_exception_handler rejects non-callables."""

import asyncio


loop = asyncio.new_event_loop()
try:
    asyncio.set_event_loop(loop)
    for value in (123, object(), "x"):
        try:
            loop.set_exception_handler(value)
        except Exception as exc:
            # The message ends with the handler's repr, which for object()
            # carries an address; compare it rather than print it.
            expected = f"A callable object or None is expected, got {value!r}"
            print(type(exc).__name__, str(exc) == expected)
finally:
    loop.close()
    asyncio.set_event_loop(None)
