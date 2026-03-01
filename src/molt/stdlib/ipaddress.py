"""Minimal ipaddress support for Molt — delegates to Rust intrinsics."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "ip_address",
    "ip_network",
    "IPv4Address",
    "IPv6Address",
    "IPv4Network",
]

# ---------------------------------------------------------------------------
# Intrinsic bindings — IPv4Address
# ---------------------------------------------------------------------------
_molt_ipaddress_v4_new = _require_intrinsic("molt_ipaddress_v4_new", globals())
_molt_ipaddress_v4_str = _require_intrinsic("molt_ipaddress_v4_str", globals())
_molt_ipaddress_v4_int = _require_intrinsic("molt_ipaddress_v4_int", globals())
_molt_ipaddress_v4_packed = _require_intrinsic("molt_ipaddress_v4_packed", globals())
_molt_ipaddress_v4_version = _require_intrinsic("molt_ipaddress_v4_version", globals())
_molt_ipaddress_v4_is_private = _require_intrinsic(
    "molt_ipaddress_v4_is_private", globals()
)
_molt_ipaddress_v4_is_loopback = _require_intrinsic(
    "molt_ipaddress_v4_is_loopback", globals()
)
_molt_ipaddress_v4_is_multicast = _require_intrinsic(
    "molt_ipaddress_v4_is_multicast", globals()
)
_molt_ipaddress_v4_is_link_local = _require_intrinsic(
    "molt_ipaddress_v4_is_link_local", globals()
)
_molt_ipaddress_v4_is_reserved = _require_intrinsic(
    "molt_ipaddress_v4_is_reserved", globals()
)
_molt_ipaddress_v4_is_global = _require_intrinsic(
    "molt_ipaddress_v4_is_global", globals()
)
_molt_ipaddress_v4_max_prefixlen = _require_intrinsic(
    "molt_ipaddress_v4_max_prefixlen", globals()
)
_molt_ipaddress_drop = _require_intrinsic("molt_ipaddress_drop", globals())

# ---------------------------------------------------------------------------
# Intrinsic bindings — IPv6Address
# ---------------------------------------------------------------------------
_molt_ipaddress_v6_new = _require_intrinsic("molt_ipaddress_v6_new", globals())
_molt_ipaddress_v6_str = _require_intrinsic("molt_ipaddress_v6_str", globals())
_molt_ipaddress_v6_int = _require_intrinsic("molt_ipaddress_v6_int", globals())
_molt_ipaddress_v6_packed = _require_intrinsic("molt_ipaddress_v6_packed", globals())
_molt_ipaddress_v6_version = _require_intrinsic("molt_ipaddress_v6_version", globals())
_molt_ipaddress_v6_is_private = _require_intrinsic(
    "molt_ipaddress_v6_is_private", globals()
)
_molt_ipaddress_v6_is_loopback = _require_intrinsic(
    "molt_ipaddress_v6_is_loopback", globals()
)
_molt_ipaddress_v6_is_multicast = _require_intrinsic(
    "molt_ipaddress_v6_is_multicast", globals()
)
_molt_ipaddress_v6_is_link_local = _require_intrinsic(
    "molt_ipaddress_v6_is_link_local", globals()
)
_molt_ipaddress_v6_is_global = _require_intrinsic(
    "molt_ipaddress_v6_is_global", globals()
)
_molt_ipaddress_v6_drop = _require_intrinsic("molt_ipaddress_v6_drop", globals())

# ---------------------------------------------------------------------------
# Intrinsic bindings — IPv4Network
# ---------------------------------------------------------------------------
_molt_ipaddress_v4_network_new = _require_intrinsic(
    "molt_ipaddress_v4_network_new", globals()
)
_molt_ipaddress_v4_network_str = _require_intrinsic(
    "molt_ipaddress_v4_network_str", globals()
)
_molt_ipaddress_v4_network_prefixlen = _require_intrinsic(
    "molt_ipaddress_v4_network_prefixlen", globals()
)
_molt_ipaddress_v4_network_broadcast = _require_intrinsic(
    "molt_ipaddress_v4_network_broadcast", globals()
)
_molt_ipaddress_v4_network_hosts = _require_intrinsic(
    "molt_ipaddress_v4_network_hosts", globals()
)
_molt_ipaddress_v4_network_contains = _require_intrinsic(
    "molt_ipaddress_v4_network_contains", globals()
)
_molt_ipaddress_v4_network_drop = _require_intrinsic(
    "molt_ipaddress_v4_network_drop", globals()
)


# ---------------------------------------------------------------------------
# Public classes
# ---------------------------------------------------------------------------


class IPv4Address:
    """IPv4 address backed by a Rust handle."""

    __slots__ = ("_handle",)

    def __init__(self, addr: str | int | bytes) -> None:
        self._handle = _molt_ipaddress_v4_new(addr)

    @classmethod
    def _from_handle(cls, handle: object) -> "IPv4Address":
        """Wrap a pre-existing Rust Ipv4Handle without re-parsing."""
        obj = object.__new__(cls)
        obj._handle = handle
        return obj

    @property
    def version(self) -> int:
        return int(_molt_ipaddress_v4_version(self._handle))

    @property
    def max_prefixlen(self) -> int:
        return int(_molt_ipaddress_v4_max_prefixlen(self._handle))

    @property
    def packed(self) -> bytes:
        return bytes(_molt_ipaddress_v4_packed(self._handle))

    @property
    def is_private(self) -> bool:
        return bool(_molt_ipaddress_v4_is_private(self._handle))

    @property
    def is_loopback(self) -> bool:
        return bool(_molt_ipaddress_v4_is_loopback(self._handle))

    @property
    def is_multicast(self) -> bool:
        return bool(_molt_ipaddress_v4_is_multicast(self._handle))

    @property
    def is_link_local(self) -> bool:
        return bool(_molt_ipaddress_v4_is_link_local(self._handle))

    @property
    def is_reserved(self) -> bool:
        return bool(_molt_ipaddress_v4_is_reserved(self._handle))

    @property
    def is_global(self) -> bool:
        return bool(_molt_ipaddress_v4_is_global(self._handle))

    def __int__(self) -> int:
        return int(_molt_ipaddress_v4_int(self._handle))

    def __str__(self) -> str:
        return str(_molt_ipaddress_v4_str(self._handle))

    def __repr__(self) -> str:
        return f"IPv4Address('{self}')"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, IPv4Address):
            return NotImplemented
        return int(self) == int(other)

    def __hash__(self) -> int:
        return hash(int(self))

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, IPv4Address):
            return NotImplemented
        return int(self) < int(other)

    def __del__(self) -> None:
        try:
            _molt_ipaddress_drop(self._handle)
        except Exception:
            pass


class IPv6Address:
    """IPv6 address backed by a Rust handle."""

    __slots__ = ("_handle",)

    def __init__(self, addr: str | int | bytes) -> None:
        self._handle = _molt_ipaddress_v6_new(addr)

    @property
    def version(self) -> int:
        return int(_molt_ipaddress_v6_version(self._handle))

    @property
    def max_prefixlen(self) -> int:
        return 128

    @property
    def packed(self) -> bytes:
        return bytes(_molt_ipaddress_v6_packed(self._handle))

    @property
    def is_private(self) -> bool:
        return bool(_molt_ipaddress_v6_is_private(self._handle))

    @property
    def is_loopback(self) -> bool:
        return bool(_molt_ipaddress_v6_is_loopback(self._handle))

    @property
    def is_multicast(self) -> bool:
        return bool(_molt_ipaddress_v6_is_multicast(self._handle))

    @property
    def is_link_local(self) -> bool:
        return bool(_molt_ipaddress_v6_is_link_local(self._handle))

    @property
    def is_global(self) -> bool:
        return bool(_molt_ipaddress_v6_is_global(self._handle))

    @property
    def compressed(self) -> str:
        return str(_molt_ipaddress_v6_str(self._handle))

    def __int__(self) -> int:
        return int(_molt_ipaddress_v6_int(self._handle))

    def __str__(self) -> str:
        return str(_molt_ipaddress_v6_str(self._handle))

    def __repr__(self) -> str:
        return f"IPv6Address('{self}')"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, IPv6Address):
            return NotImplemented
        return int(self) == int(other)

    def __hash__(self) -> int:
        return hash(int(self))

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, IPv6Address):
            return NotImplemented
        return int(self) < int(other)

    def __del__(self) -> None:
        try:
            _molt_ipaddress_v6_drop(self._handle)
        except Exception:
            pass


class IPv4Network:
    """IPv4 network backed by a Rust handle."""

    __slots__ = ("_handle",)

    def __init__(self, addr: str | tuple[str | int, int], strict: bool = True) -> None:
        self._handle = _molt_ipaddress_v4_network_new(addr, strict)

    @property
    def prefixlen(self) -> int:
        return int(_molt_ipaddress_v4_network_prefixlen(self._handle))

    @property
    def num_addresses(self) -> int:
        return 1 << (32 - self.prefixlen)

    @property
    def network_address(self) -> IPv4Address:
        # The network address string is the prefix portion of the CIDR str.
        cidr = str(_molt_ipaddress_v4_network_str(self._handle))
        host_str = cidr.split("/", 1)[0]
        return IPv4Address(host_str)

    @property
    def broadcast_address(self) -> IPv4Address:
        return IPv4Address._from_handle(
            _molt_ipaddress_v4_network_broadcast(self._handle)
        )

    def hosts(self) -> list[IPv4Address]:
        raw = _molt_ipaddress_v4_network_hosts(self._handle)
        # The intrinsic returns a list of Ipv4Handle bits; wrap each directly.
        result: list[IPv4Address] = []
        for handle in raw:
            result.append(IPv4Address._from_handle(handle))
        return result

    def __contains__(self, addr: object) -> bool:
        if not isinstance(addr, IPv4Address):
            return False
        return bool(_molt_ipaddress_v4_network_contains(self._handle, addr._handle))

    def __str__(self) -> str:
        return str(_molt_ipaddress_v4_network_str(self._handle))

    def __repr__(self) -> str:
        return f"IPv4Network('{self}')"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, IPv4Network):
            return NotImplemented
        return str(self) == str(other)

    def __hash__(self) -> int:
        return hash(str(self))

    def __del__(self) -> None:
        try:
            _molt_ipaddress_v4_network_drop(self._handle)
        except Exception:
            pass


# ---------------------------------------------------------------------------
# Module-level helpers
# ---------------------------------------------------------------------------


def ip_address(address: str | int | bytes) -> IPv4Address | IPv6Address:
    """Return an IPv4Address or IPv6Address depending on *address*."""
    if isinstance(address, str) and ":" in address:
        return IPv6Address(address)
    if isinstance(address, int):
        if address < 0:
            raise ValueError("negative address")
        if address <= 0xFFFFFFFF:
            return IPv4Address(address)
        if address <= (1 << 128) - 1:
            return IPv6Address(address)
        raise ValueError("address out of range")
    if isinstance(address, (bytes, bytearray)):
        if len(address) == 4:
            return IPv4Address(address)
        if len(address) == 16:
            return IPv6Address(address)
        raise ValueError("address must be 4 or 16 bytes")
    return IPv4Address(address)


def ip_network(
    address: str | tuple[str | int, int], strict: bool = True
) -> IPv4Network:
    """Return an IPv4Network for *address*."""
    return IPv4Network(address, strict)
