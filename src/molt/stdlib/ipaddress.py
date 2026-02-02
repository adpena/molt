"""Minimal ipaddress support for Molt."""

from __future__ import annotations

__all__ = [
    "ip_address",
    "ip_network",
    "IPv4Address",
    "IPv6Address",
    "IPv4Network",
]


def _parse_ipv4(text: str) -> int:
    parts = text.split(".")
    if len(parts) != 4:
        raise ValueError("invalid IPv4 address")
    nums: list[int] = []
    for part in parts:
        if not part or not part.isdigit():
            raise ValueError("invalid IPv4 address")
        val = int(part)
        if val < 0 or val > 255:
            raise ValueError("invalid IPv4 address")
        nums.append(val)
    out = 0
    for val in nums:
        out = (out << 8) | val
    return out


def _parse_ipv6(text: str) -> int:
    if "::" in text:
        head, tail = text.split("::", 1)
        head_parts = [p for p in head.split(":") if p]
        tail_parts = [p for p in tail.split(":") if p]
        missing = 8 - (len(head_parts) + len(tail_parts))
        if missing < 0:
            raise ValueError("invalid IPv6 address")
        parts = head_parts + (["0"] * missing) + tail_parts
    else:
        parts = text.split(":")
        if len(parts) != 8:
            raise ValueError("invalid IPv6 address")
    if len(parts) != 8:
        raise ValueError("invalid IPv6 address")
    nums: list[int] = []
    for part in parts:
        if not part:
            part = "0"
        if len(part) > 4:
            raise ValueError("invalid IPv6 address")
        try:
            val = int(part, 16)
        except ValueError as exc:
            raise ValueError("invalid IPv6 address") from exc
        if val < 0 or val > 0xFFFF:
            raise ValueError("invalid IPv6 address")
        nums.append(val)
    out = 0
    for val in nums:
        out = (out << 16) | val
    return out


def _compress_ipv6(words: list[int]) -> str:
    best_start = -1
    best_len = 0
    cur_start = -1
    cur_len = 0
    for idx, val in enumerate(words + [1]):
        if val == 0 and idx < 8:
            if cur_start == -1:
                cur_start = idx
                cur_len = 1
            else:
                cur_len += 1
        else:
            if cur_len > best_len and cur_len > 1:
                best_start = cur_start
                best_len = cur_len
            cur_start = -1
            cur_len = 0
    if best_start == -1:
        return ":".join(f"{w:x}" for w in words)
    head = ":".join(f"{w:x}" for w in words[:best_start])
    tail = ":".join(f"{w:x}" for w in words[best_start + best_len :])
    if head and tail:
        return f"{head}::{tail}"
    if head:
        return f"{head}::"
    if tail:
        return f"::{tail}"
    return "::"


class IPv4Address:
    def __init__(self, addr: str | int):
        if isinstance(addr, int):
            if addr < 0 or addr > 0xFFFFFFFF:
                raise ValueError("invalid IPv4 address")
            self._value = addr
        else:
            self._value = _parse_ipv4(addr)

    @property
    def version(self) -> int:
        return 4

    @property
    def packed(self) -> bytes:
        return self._value.to_bytes(4, "big")

    @property
    def is_private(self) -> bool:
        val = self._value
        if (val >> 24) == 10:
            return True
        if (val >> 20) == 0xAC1:
            return True
        if (val >> 16) == 0xC0A8:
            return True
        return False

    def __str__(self) -> str:
        parts = [str((self._value >> shift) & 0xFF) for shift in (24, 16, 8, 0)]
        return ".".join(parts)

    def __repr__(self) -> str:
        return f"IPv4Address('{self}')"


class IPv6Address:
    def __init__(self, addr: str | int):
        if isinstance(addr, int):
            if addr < 0 or addr > (1 << 128) - 1:
                raise ValueError("invalid IPv6 address")
            self._value = addr
        else:
            self._value = _parse_ipv6(addr)

    @property
    def version(self) -> int:
        return 6

    @property
    def packed(self) -> bytes:
        return self._value.to_bytes(16, "big")

    @property
    def is_private(self) -> bool:
        prefix = self._value >> 121
        if prefix in (0b1111110, 0b1111111):
            return True
        if (self._value >> 118) == 0b1111111010:
            return True
        return False

    @property
    def compressed(self) -> str:
        words = [(self._value >> shift) & 0xFFFF for shift in range(112, -1, -16)]
        return _compress_ipv6(words)

    def __str__(self) -> str:
        return self.compressed

    def __repr__(self) -> str:
        return f"IPv6Address('{self.compressed}')"


class IPv4Network:
    def __init__(self, addr: str):
        if "/" not in addr:
            raise ValueError("invalid IPv4 network")
        ip_text, prefix_text = addr.split("/", 1)
        prefix = int(prefix_text)
        if prefix < 0 or prefix > 32:
            raise ValueError("invalid prefix")
        ip_val = _parse_ipv4(ip_text)
        mask = (0xFFFFFFFF << (32 - prefix)) & 0xFFFFFFFF
        self._network = ip_val & mask
        self._mask = mask
        self._prefix = prefix

    @property
    def num_addresses(self) -> int:
        return 1 << (32 - self._prefix)

    @property
    def network_address(self) -> IPv4Address:
        return IPv4Address(self._network)

    @property
    def broadcast_address(self) -> IPv4Address:
        return IPv4Address(self._network | (~self._mask & 0xFFFFFFFF))

    def __repr__(self) -> str:
        return f"IPv4Network('{self.network_address}/{self._prefix}')"


def ip_address(address: str) -> IPv4Address | IPv6Address:
    if ":" in address:
        return IPv6Address(address)
    return IPv4Address(address)


def ip_network(address: str) -> IPv4Network:
    return IPv4Network(address)
