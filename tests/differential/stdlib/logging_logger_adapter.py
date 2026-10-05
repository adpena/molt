"""Purpose: differential coverage for logging logger adapter."""

import io
import logging
import weakref


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(message)s %(user)s"))

logger = logging.getLogger("molt.adapter")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False

adapter = logging.LoggerAdapter(logger, {"user": "alice"})
adapter.info("hello")
handler.flush()

print(stream.getvalue().strip())

logger.extension_marker = "inherited-dict"
print(logger.__dict__["extension_marker"], weakref.ref(logger)() is logger)
