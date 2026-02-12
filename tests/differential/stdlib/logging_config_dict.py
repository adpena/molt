"""Purpose: differential coverage for logging config dict."""

import logging
import logging.config


config = {
    "version": 1,
    "disable_existing_loggers": False,
    "formatters": {
        "simple": {"format": "%(levelname)s:%(message)s"},
    },
    "handlers": {
        "console": {
            "class": "logging.StreamHandler",
            "level": "INFO",
            "formatter": "simple",
            "stream": "ext://sys.stdout",
        },
    },
    "loggers": {
        "molt": {"handlers": ["console"], "level": "INFO"},
    },
}

logging.config.dictConfig(config)
logger = logging.getLogger("molt")
logger.info("hello")
