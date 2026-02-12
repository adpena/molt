"""Planned parity coverage for optional hashlib algorithms."""

from __future__ import annotations

import hashlib
import hmac


def show(label: str, value) -> None:
    print(f"{label}: {value}")


PAYLOAD = b"molt-optional-hash"

for name in ("sha512_224", "sha512_256"):
    h = hashlib.new(name, PAYLOAD)
    show(f"new:{name}:hexdigest", h.hexdigest())
    show(f"new:{name}:sizes", (h.digest_size, h.block_size))

show("ctor:sha512_224", hashlib.sha512_224(PAYLOAD).hexdigest())
show("ctor:sha512_256", hashlib.sha512_256(PAYLOAD).hexdigest())

for alias in ("sha512-224", "sha512/224", "sha512-256", "sha512/256"):
    show(f"alias:{alias}", hashlib.new(alias, PAYLOAD).hexdigest())

for digest_name in ("sha512_224", "sha512_256"):
    derived = hashlib.pbkdf2_hmac(digest_name, b"password", b"salt", 1024, 24)
    show(f"pbkdf2:{digest_name}", derived.hex())

show("hmac:sha512_224", hmac.new(b"k", b"v", digestmod="sha512_224").hexdigest())
show("hmac:sha512_256", hmac.new(b"k", b"v", digestmod="sha512_256").hexdigest())
