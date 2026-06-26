"""Transport authority for `asyncio.transports`."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")

if TYPE_CHECKING:
    from .protocols import BaseProtocol

class BaseTransport:
    """Base class for transports."""

    def __init__(self, extra: dict | None = None):
        self._extra = extra if extra is not None else {}

    def get_extra_info(self, name: str, default: Any = None) -> Any:
        return self._extra.get(name, default)

    def is_closing(self) -> bool:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError

    def set_protocol(self, protocol: "BaseProtocol") -> None:
        raise NotImplementedError

    def get_protocol(self) -> "BaseProtocol":
        raise NotImplementedError

class ReadTransport(BaseTransport):
    """Interface for read-only transports."""

    def is_reading(self) -> bool:
        raise NotImplementedError

    def pause_reading(self) -> None:
        raise NotImplementedError

    def resume_reading(self) -> None:
        raise NotImplementedError

class WriteTransport(BaseTransport):
    """Interface for write-only transports."""

    def set_write_buffer_limits(
        self, high: int | None = None, low: int | None = None
    ) -> None:
        raise NotImplementedError

    def get_write_buffer_size(self) -> int:
        raise NotImplementedError

    def get_write_buffer_limits(self) -> tuple[int, int]:
        raise NotImplementedError

    def write(self, data: bytes) -> None:
        raise NotImplementedError

    def writelines(self, list_of_data: list[bytes]) -> None:
        for data in list_of_data:
            self.write(data)

    def write_eof(self) -> None:
        raise NotImplementedError

    def can_write_eof(self) -> bool:
        raise NotImplementedError

    def abort(self) -> None:
        raise NotImplementedError

class Transport(ReadTransport, WriteTransport):
    """Interface representing a bidirectional transport."""

class DatagramTransport(BaseTransport):
    """Interface for datagram (UDP) transports."""

    def sendto(self, data: bytes, addr: Any = None) -> None:
        raise NotImplementedError

    def abort(self) -> None:
        raise NotImplementedError

class SubprocessTransport(BaseTransport):
    """Interface for subprocess transports."""

    def get_pid(self) -> int:
        raise NotImplementedError

    def get_returncode(self) -> int | None:
        raise NotImplementedError

    def get_pipe_transport(self, fd: int) -> BaseTransport | None:
        raise NotImplementedError

    def send_signal(self, signal: int) -> None:
        raise NotImplementedError

    def terminate(self) -> None:
        raise NotImplementedError

    def kill(self) -> None:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError

__all__ = [
    "BaseTransport",
    "DatagramTransport",
    "ReadTransport",
    "SubprocessTransport",
    "Transport",
    "WriteTransport",
]

globals().pop("_require_intrinsic", None)
