"""Purpose: differential coverage for logging filter mutation."""

import io
import logging


stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.setFormatter(logging.Formatter("%(message)s"))

logger = logging.getLogger("molt.filter")
logger.handlers[:] = []
logger.setLevel(logging.INFO)
logger.addHandler(handler)
logger.propagate = False


class MutateFilter(logging.Filter):
    def filter(self, record):
        record.msg = f"mutated:{record.getMessage()}"
        return True


logger.addFilter(MutateFilter())
logger.info("hello")
handler.flush()

print(stream.getvalue().strip())
