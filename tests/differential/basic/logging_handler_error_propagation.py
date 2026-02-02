"""Purpose: differential coverage for logging handler error propagation."""

import logging

class BadHandler(logging.Handler):
    def emit(self, record):
        raise RuntimeError("boom")

logger = logging.getLogger("err_demo")
logger.handlers[:] = [BadHandler()]
logger.setLevel(logging.INFO)
logger.propagate = False

try:
    logger.info("hi")
    print("ok")
except Exception as exc:
    print("err", type(exc).__name__)
