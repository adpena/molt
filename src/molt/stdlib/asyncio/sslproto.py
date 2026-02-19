"""Public API surface shim for ``asyncio.sslproto``."""

from __future__ import annotations

import collections
import enum
import ssl
import warnings

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

import asyncio.constants as constants
import asyncio.exceptions as exceptions
import asyncio.protocols as protocols
import asyncio.transports as transports
from asyncio.log import logger


class EnumType(type):
    pass


class AppProtocolState(metaclass=EnumType):
    STATE_INIT = 0
    STATE_CON_MADE = 1
    STATE_EOF = 2
    STATE_CON_LOST = 3


class SSLProtocolState(metaclass=EnumType):
    UNWRAPPED = 0
    DO_HANDSHAKE = 1
    WRAPPED = 2
    FLUSHING = 3
    SHUTDOWN = 4


class SSLProtocol:
    pass


del EnumType


_ssl_again_errors = []
for _name in ("SSLWantReadError", "SSLWantWriteError"):
    _value = getattr(ssl, _name, None)
    if isinstance(_value, type):
        _ssl_again_errors.append(_value)
if not _ssl_again_errors:
    _ssl_again_errors = [ssl.SSLError]
SSLAgainErrors = tuple(_ssl_again_errors)


def add_flowcontrol_defaults(*args, **kwargs):
    return args, kwargs


__all__ = [
    "AppProtocolState",
    "SSLAgainErrors",
    "SSLProtocol",
    "SSLProtocolState",
    "add_flowcontrol_defaults",
    "collections",
    "constants",
    "enum",
    "exceptions",
    "logger",
    "protocols",
    "ssl",
    "transports",
    "warnings",
]
