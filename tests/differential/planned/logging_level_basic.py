"""Purpose: differential coverage for logging level/format basics."""

import logging

logger = logging.getLogger("molt")
logger.setLevel(logging.INFO)
print(logger.isEnabledFor(logging.INFO))
