"""Protocol authority for `asyncio.protocols`."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

if TYPE_CHECKING:
    from .transports import BaseTransport

class BaseProtocol:
    """Base class for protocols."""

    def connection_made(self, transport: BaseTransport) -> None:
        """Called when a connection is made."""

    def connection_lost(self, exc: BaseException | None) -> None:
        """Called when the connection is lost or closed."""

    def pause_writing(self) -> None:
        """Called when the transport's buffer goes over the high-water mark."""

    def resume_writing(self) -> None:
        """Called when the transport's buffer drains below the low-water mark."""

class Protocol(BaseProtocol):
    """Interface for stream protocol event callbacks."""

    def data_received(self, data: bytes) -> None:
        """Called when some data is received."""

    def eof_received(self) -> bool | None:
        """Called when the other end signals it won't send data anymore."""

class BufferedProtocol(BaseProtocol):
    """Interface for stream protocol with manual buffer control."""

    def get_buffer(self, sizehint: int) -> bytearray:
        """Called to allocate a new receive buffer."""
        raise NotImplementedError

    def buffer_updated(self, nbytes: int) -> None:
        """Called when the buffer was updated with the received data."""
        raise NotImplementedError

    def eof_received(self) -> bool | None:
        """Called when the other end signals it won't send data anymore."""

class DatagramProtocol(BaseProtocol):
    """Interface for datagram protocol event callbacks."""

    def datagram_received(self, data: bytes, addr: Any) -> None:
        """Called when a datagram is received."""

    def error_received(self, exc: OSError) -> None:
        """Called when a send or receive operation raises an OSError."""

class SubprocessProtocol(BaseProtocol):
    """Interface for subprocess event callbacks."""

    def pipe_data_received(self, fd: int, data: bytes) -> None:
        """Called when the child process writes data into its stdout or stderr pipe."""

    def pipe_connection_lost(self, fd: int, exc: BaseException | None) -> None:
        """Called when one of the pipes communicating with the child process is closed."""

    def process_exited(self) -> None:
        """Called when the child process has exited."""

__all__ = [
    "BaseProtocol",
    "BufferedProtocol",
    "DatagramProtocol",
    "Protocol",
    "SubprocessProtocol",
]

globals().pop("_require_intrinsic", None)
