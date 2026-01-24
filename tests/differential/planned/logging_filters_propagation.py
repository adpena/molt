"""Purpose: differential coverage for logging filters and propagation."""

import io
import logging

stream = io.StringIO()
handler = logging.StreamHandler(stream)

class Dropper(logging.Filter):
    def filter(self, record):
        return record.msg != "drop"

logger = logging.getLogger("filter_demo")
logger.handlers[:] = [handler]
logger.setLevel(logging.INFO)
logger.propagate = False
logger.addFilter(Dropper())

logger.info("keep")
logger.info("drop")

print("out", stream.getvalue().strip())
