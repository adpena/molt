"""Purpose: differential coverage for logging handler exception."""

import io
import logging


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(message)s"))

logger = logging.getLogger("molt.handler")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False


class BadHandler(logging.Handler):
    def emit(self, record):
        raise RuntimeError("emit failed")


bad = BadHandler()
logger.addHandler(bad)

try:
    logger.info("hello")
except Exception as exc:
    print(type(exc).__name__)

logger.removeHandler(bad)
logger.info("ok")
handler.flush()
print(stream.getvalue().strip().splitlines()[-1])
