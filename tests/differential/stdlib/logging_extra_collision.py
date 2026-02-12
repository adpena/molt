"""Purpose: differential coverage for logging extra collision."""

import io
import logging


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))

logger = logging.getLogger("molt.extra")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False

try:
    logger.info("hi", extra={"message": "override"})
except Exception as exc:
    print(type(exc).__name__)

logger.info("ok")
handler.flush()
print(stream.getvalue().strip().splitlines()[-1])
