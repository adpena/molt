"""Purpose: differential coverage for logging format errors."""

import io
import logging


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(missing)s"))
logger = logging.getLogger("molt.format")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False

try:
    logger.info("hello")
except Exception as exc:
    print(type(exc).__name__)

handler.setFormatter(logging.Formatter("%(message)s"))
logger.info("ok")
handler.flush()

print(stream.getvalue().strip().splitlines()[-1])
