"""Purpose: differential coverage for logging Formatter + stack_info."""

import io
import logging

stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(message)s"))
logger = logging.getLogger("stack_demo")
logger.setLevel(logging.INFO)
logger.handlers[:] = [handler]
logger.propagate = False

logger.info("hello", stack_info=True)
print("out", stream.getvalue().splitlines()[0])
