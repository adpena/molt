# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,os.getpid
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


# The constructor may remove the only configuration reference to its mode.
# Re-reading that dictionary after the callback cannot determine ownership.
lifetime_events = []
active_config = {}


class Mode(str):
    def __del__(self):
        lifetime_events.append("mode retired")


class MutatingFileHandler(logging.Handler):
    def __init__(self, filename, mode="a"):
        active_config["handlers"]["file"].pop("mode", None)
        lifetime_events.append(("constructor", filename, str(mode)))
        assert mode == "w"
        super().__init__()


original_file_handler = logging.FileHandler
logging.FileHandler = MutatingFileHandler
try:
    active_config = {
        "version": 1,
        "disable_existing_loggers": False,
        "handlers": {
            "file": {
                "class": "logging.FileHandler",
                "filename": "constructor-probe-does-not-open-a-file",
                "mode": Mode("w"),
            }
        },
        "loggers": {"molt-lifetime": {"handlers": ["file"], "level": "INFO"}},
    }
    logging.config.dictConfig(active_config)
    assert lifetime_events == [
        ("constructor", "constructor-probe-does-not-open-a-file", "w"),
        "mode retired",
    ]
    assert "mode" not in active_config["handlers"]["file"]
    print("configuration callback lifetime", lifetime_events)
finally:
    logging.FileHandler = original_file_handler
    probe_logger = logging.getLogger("molt-lifetime")
    for handler in probe_logger.handlers[:]:
        probe_logger.removeHandler(handler)
        handler.close()
