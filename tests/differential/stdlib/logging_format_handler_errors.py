"""Purpose: differential coverage for logging formatting and handler errors."""

import io
import logging

stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))
logger = logging.getLogger("demo")
logger.setLevel(logging.INFO)
logger.handlers[:] = [handler]
logger.propagate = False

logger.info("hello %s", "world")
print("line", stream.getvalue().strip())

try:
    bad = logging.Formatter("%(missing)s")
    handler.setFormatter(bad)
    logger.info("oops")
except Exception as exc:
    print("error", type(exc).__name__)
