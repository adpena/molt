"""Compiler backend transport facts shared with the compiled server."""

from pathlib import Path
from typing import Literal


BACKEND_DAEMON_PROTOCOL_VERSION = int(
    Path(__file__).with_name("backend_daemon_protocol_version.txt").read_text("ascii")
)
if not 0 < BACKEND_DAEMON_PROTOCOL_VERSION < 1 << 32:
    raise ValueError("backend daemon protocol version must fit a positive u32")

BACKEND_IR_LEASE_FORMAT: Literal["msgpack"] = "msgpack"
