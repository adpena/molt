"""Intrinsic-backed `_blake2` compatibility surface."""

from __future__ import annotations

import hashlib as _hashlib
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has")

_Hash = _hashlib._Hash

_GIL_MINSIZE = 2048

BLAKE2B_MAX_DIGEST_SIZE = 64
BLAKE2B_MAX_KEY_SIZE = 64
BLAKE2B_SALT_SIZE = 16
BLAKE2B_PERSON_SIZE = 16

BLAKE2S_MAX_DIGEST_SIZE = 32
BLAKE2S_MAX_KEY_SIZE = 32
BLAKE2S_SALT_SIZE = 8
BLAKE2S_PERSON_SIZE = 8


class blake2b(_Hash):
    def __init__(
        self,
        data: Any = b"",
        *,
        digest_size: int = BLAKE2B_MAX_DIGEST_SIZE,
        key: Any = b"",
        salt: Any = b"",
        person: Any = b"",
        fanout: int = 1,
        depth: int = 1,
        leaf_size: int = 0,
        node_offset: int = 0,
        node_depth: int = 0,
        inner_size: int = 0,
        last_node: bool = False,
        usedforsecurity: bool = True,
    ) -> None:
        options = {
            "digest_size": digest_size,
            "key": key,
            "salt": salt,
            "person": person,
            "fanout": fanout,
            "depth": depth,
            "leaf_size": leaf_size,
            "node_offset": node_offset,
            "node_depth": node_depth,
            "inner_size": inner_size,
            "last_node": last_node,
            "usedforsecurity": usedforsecurity,
        }
        super().__init__(
            "blake2b", data, _hashlib._validate_options("blake2b", options, "blake2b")
        )


class blake2s(_Hash):
    def __init__(
        self,
        data: Any = b"",
        *,
        digest_size: int = BLAKE2S_MAX_DIGEST_SIZE,
        key: Any = b"",
        salt: Any = b"",
        person: Any = b"",
        fanout: int = 1,
        depth: int = 1,
        leaf_size: int = 0,
        node_offset: int = 0,
        node_depth: int = 0,
        inner_size: int = 0,
        last_node: bool = False,
        usedforsecurity: bool = True,
    ) -> None:
        options = {
            "digest_size": digest_size,
            "key": key,
            "salt": salt,
            "person": person,
            "fanout": fanout,
            "depth": depth,
            "leaf_size": leaf_size,
            "node_offset": node_offset,
            "node_depth": node_depth,
            "inner_size": inner_size,
            "last_node": last_node,
            "usedforsecurity": usedforsecurity,
        }
        super().__init__(
            "blake2s", data, _hashlib._validate_options("blake2s", options, "blake2s")
        )


globals().pop("_require_intrinsic", None)
