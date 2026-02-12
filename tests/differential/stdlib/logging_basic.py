"""Purpose: differential coverage for logging basic."""

import io
import logging


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(levelname)s:%(message)s"))

logger = logging.getLogger("molt.test")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False

logger.debug("skip")
logger.info("hello")
handler.flush()

print(stream.getvalue().strip())
