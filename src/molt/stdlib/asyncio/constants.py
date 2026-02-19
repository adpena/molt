"""CPython 3.12-compatible `asyncio.constants` surface."""

import enum

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())

LOG_THRESHOLD_FOR_CONNLOST_WRITES = 5
ACCEPT_RETRY_DELAY = 1
DEBUG_STACK_DEPTH = 10
SSL_HANDSHAKE_TIMEOUT = 60.0
SSL_SHUTDOWN_TIMEOUT = 30.0
SENDFILE_FALLBACK_READBUFFER_SIZE = 256 * 1024
FLOW_CONTROL_HIGH_WATER_SSL_READ = 256
FLOW_CONTROL_HIGH_WATER_SSL_WRITE = 512
THREAD_JOIN_TIMEOUT = 300


class _SendfileMode(enum.Enum):
    UNSUPPORTED = enum.auto()
    TRY_NATIVE = enum.auto()
    FALLBACK = enum.auto()
