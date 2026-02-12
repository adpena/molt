"""Purpose: differential coverage for logging propagate toggle."""

import io
import logging


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(name)s:%(message)s"))

root = logging.getLogger()
root.handlers[:] = []
root.setLevel(logging.INFO)
root.addHandler(handler)

logger = logging.getLogger("molt.toggle")
logger.setLevel(logging.INFO)

logger.propagate = False
logger.info("nope")

logger.propagate = True
logger.info("yep")
handler.flush()

print(stream.getvalue().strip().splitlines())
