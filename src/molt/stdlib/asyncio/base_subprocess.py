"""Public API surface shim for ``asyncio.base_subprocess``."""

from __future__ import annotations

import collections
import logging as _logging
import subprocess
import warnings


from . import protocols
from . import transports


logger = _logging.getLogger("asyncio")


class BaseSubprocessTransport:
    pass


class ReadSubprocessPipeProto:
    pass


class WriteSubprocessPipeProto:
    pass


__all__ = [
    "BaseSubprocessTransport",
    "ReadSubprocessPipeProto",
    "WriteSubprocessPipeProto",
    "collections",
    "logger",
    "protocols",
    "subprocess",
    "transports",
    "warnings",
]
