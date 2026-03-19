"""Intrinsic-backed config helpers for the stdlib ``logging`` package."""

from __future__ import annotations

from typing import Any as _Any

import errno
import functools
import io
import logging
import os
import queue
import re
import struct
import threading
import traceback
import socketserver as _socketserver

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

_MOLT_LOGGING_CONFIG_DICT = _require_intrinsic("molt_logging_config_dict")
_MOLT_LOGGING_CONFIG_VALID_IDENT = _require_intrinsic(
    "molt_logging_config_valid_ident"
)
_MOLT_LOGGING_CONFIG_FILE_CONFIG = _require_intrinsic(
    "molt_logging_config_file_config"
)
_MOLT_LOGGING_CONFIG_LISTEN = _require_intrinsic(
    "molt_logging_config_listen"
)
_MOLT_LOGGING_CONFIG_STOP_LISTENING = _require_intrinsic(
    "molt_logging_config_stop_listening"
)

DEFAULT_LOGGING_CONFIG_PORT = 9030
RESET_ERROR = errno.ECONNRESET
IDENTIFIER = re.compile(r"^[a-z_][a-z0-9_]*$", re.I)
StreamRequestHandler = getattr(
    _socketserver, "StreamRequestHandler", type("StreamRequestHandler", (), {})
)
ThreadingTCPServer = getattr(
    _socketserver, "ThreadingTCPServer", type("ThreadingTCPServer", (), {})
)

__all__ = [
    "BaseConfigurator",
    "ConvertingDict",
    "ConvertingList",
    "ConvertingMixin",
    "ConvertingTuple",
    "DEFAULT_LOGGING_CONFIG_PORT",
    "DictConfigurator",
    "IDENTIFIER",
    "RESET_ERROR",
    "StreamRequestHandler",
    "ThreadingTCPServer",
    "dictConfig",
    "dictConfigClass",
    "errno",
    "fileConfig",
    "functools",
    "io",
    "listen",
    "logging",
    "os",
    "queue",
    "re",
    "stopListening",
    "struct",
    "threading",
    "traceback",
    "valid_ident",
]


class BaseConfigurator:
    def __init__(self, config: dict[str, _Any]):
        if not isinstance(config, dict):
            raise TypeError("config must be a dict")
        self.config = config

    def configure(self) -> _Any:
        raise NotImplementedError


class DictConfigurator(BaseConfigurator):
    def configure(self) -> None:
        _MOLT_LOGGING_CONFIG_DICT(self.config)


class ConvertingMixin:
    pass


class ConvertingDict(dict, ConvertingMixin):
    pass


class ConvertingList(list, ConvertingMixin):
    pass


class ConvertingTuple(tuple, ConvertingMixin):
    pass


dictConfigClass = DictConfigurator


def dictConfig(config: dict[str, _Any]) -> None:
    dictConfigClass(config).configure()


def valid_ident(s: str) -> bool:
    return bool(_MOLT_LOGGING_CONFIG_VALID_IDENT(s))


def fileConfig(
    fname: _Any,
    defaults: dict[str, _Any] | None = None,
    disable_existing_loggers: bool = True,
    encoding: str | None = None,
) -> None:
    _MOLT_LOGGING_CONFIG_FILE_CONFIG(
        fname, defaults, bool(disable_existing_loggers), encoding
    )


def listen(
    port: int = DEFAULT_LOGGING_CONFIG_PORT,
    verify: _Any | None = None,
) -> None:
    _MOLT_LOGGING_CONFIG_LISTEN(int(port), verify)


def stopListening() -> None:
    _MOLT_LOGGING_CONFIG_STOP_LISTENING()
    return None

globals().pop("_require_intrinsic", None)
