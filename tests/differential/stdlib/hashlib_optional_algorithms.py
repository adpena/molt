"""Optional hashlib name admission, digest and buffer protocol parity."""

from __future__ import annotations

import _hashlib
import hashlib
import hmac
import sys


def show(label: str, value) -> None:
    print(f"{label}: {value}")


PAYLOAD = b"molt-optional-hash"

for name in ("sha512_224", "sha512_256"):
    h = hashlib.new(name, PAYLOAD)
    show(f"new:{name}:hexdigest", h.hexdigest())
    show(f"new:{name}:sizes", (h.digest_size, h.block_size))

show("ctor:sha512_224:present", hasattr(hashlib, "sha512_224"))
show("ctor:sha512_256:present", hasattr(hashlib, "sha512_256"))

for alias in ("sha512-224", "sha512/224", "sha512-256", "sha512/256"):
    try:
        alias_digest = hashlib.new(alias, PAYLOAD).hexdigest()
    except ValueError as error:
        show(f"alias:{alias}", (type(error).__name__, str(error)))
    else:
        show(f"alias:{alias}", alias_digest)

for digest_name in ("sha512_224", "sha512_256"):
    derived = hashlib.pbkdf2_hmac(digest_name, b"password", b"salt", 1024, 24)
    show(f"pbkdf2:{digest_name}", derived.hex())

show("hmac:sha512_224", hmac.new(b"k", b"v", digestmod="sha512_224").hexdigest())
show("hmac:sha512_256", hmac.new(b"k", b"v", digestmod="sha512_256").hexdigest())

# Named public constructors and provider algorithm names are different surfaces.
# Exercise accepted case/hyphen aliases and rejected slash spellings through
# every existing name-bearing crypto entrypoint, without hiding other failures.
for name in (
    "SHA512_224", "sha512-224", "sha512/224", "sha-512/224",
    "SHA512_256", "sha512-256", "sha512/256", "sha-512/256",
    "Sha512_224", "SHA3_256", "sha256 ", " sha256", "sha256\t",
    "sha3_256 ", "BLAKE2B ",
):
    for role, operation in (
        ("new", lambda: hashlib.new(name, PAYLOAD).digest()),
        ("private-new", lambda: _hashlib.new(name, PAYLOAD).digest()),
        ("hmac-new", lambda: hmac.new(b"k", b"v", name).digest()),
        ("hmac-digest", lambda: hmac.digest(b"k", b"v", name)),
        ("private-hmac-new", lambda: _hashlib.hmac_new(b"k", b"v", name).digest()),
        ("private-hmac-digest", lambda: _hashlib.hmac_digest(b"k", b"v", name)),
        ("pbkdf2", lambda: hashlib.pbkdf2_hmac(name, b"password", b"salt", 1, 24)),
    ):
        try:
            digest = operation()
        except ValueError as error:
            show(f"name:{name}:{role}", (type(error).__name__, str(error)))
        else:
            show(f"name:{name}:{role}", digest.hex())

for name in ("SHA-512/224", "sha256 ", "sha3-256"):
    value = hashlib.new(name, PAYLOAD)
    duplicate = value.copy()
    show("canonical:" + repr(name), (value.name, value.digest_size, value.block_size,
                                    duplicate.name, duplicate.digest() == value.digest()))
for name in ("BLAKE2B", "Blake2S"):
    for role, operation in (
        ("new", lambda: hashlib.new(name, PAYLOAD)),
        ("private-new", lambda: _hashlib.new(name, PAYLOAD)),
        ("hmac", lambda: hmac.new(b"k", b"v", name)),
        ("private-hmac", lambda: _hashlib.hmac_new(b"k", b"v", name)),
    ):
        try:
            value = operation()
        except ValueError as error:
            show("case:" + name + ":" + role, (type(error).__name__, str(error)))
        else:
            show("case:" + name + ":" + role, (value.name, value.digest_size, value.hexdigest()))

# Protocol callbacks are observed through real exporters and public operations.
_events = []
_marker = RuntimeError("crypto callback marker")


class CryptoBuffer:
    def __init__(self, label, data, action=None):
        self.label = label
        self.data = bytearray(data)
        self.action = action

    def __buffer__(self, flags):
        _events.append((self.label, "acquire", flags))
        if self.action is not None:
            self.action()
        return memoryview(self.data)

    def __release_buffer__(self, view):
        _events.append((self.label, "release"))


class CryptoIndex:
    def __init__(self, label, value, action=None):
        self.label = label
        self.value = value
        self.action = action

    def __index__(self):
        _events.append((self.label, "index"))
        if self.action is not None:
            self.action()
        return self.value


class CryptoTruth:
    def __init__(self, label, fail=False):
        self.label = label
        self.fail = fail

    def __bool__(self):
        _events.append((self.label, "bool"))
        if self.fail:
            raise _marker
        return True


def crypto_probe(label, action):
    _events.clear()
    try:
        result = action()
        print(label, result)
    except Exception as error:
        print(label, type(error).__name__, error is _marker)
    print(label, "events", _events)


_data = CryptoBuffer("data", b"abc")
crypto_probe("hash-export", lambda: hashlib.sha256(_data).hexdigest())
crypto_probe(
    "hash-security",
    lambda: hashlib.sha256(_data, usedforsecurity=CryptoTruth("security", True)),
)
crypto_probe(
    "hash-unknown-order",
    lambda: hashlib.new(
        "molt-not-a-hash", _data, usedforsecurity=CryptoTruth("security")
    ),
)

_key = CryptoBuffer("key", b"key")
_salt = CryptoBuffer("salt", b"salt")
_person = CryptoBuffer("person", b"person")


def mutate_blake_key():
    _key.data[0] = ord("K")
    for item in (_key, _salt, _person):
        try:
            item.data.append(0)
        except BufferError:
            _events.append((item.label, "pinned"))


crypto_probe(
    "blake-conversions",
    lambda: hashlib.blake2b(
        _data,
        digest_size=CryptoIndex("size", 16),
        key=_key,
        salt=_salt,
        person=_person,
        fanout=CryptoIndex("fanout", 1, mutate_blake_key),
        last_node=CryptoTruth("last"),
        usedforsecurity=CryptoTruth("security"),
    ).hexdigest(),
)
crypto_probe(
    "blake-conversion-before-validation",
    lambda: hashlib.blake2s(
        _data,
        digest_size=0,
        key=_key,
        salt=_salt,
        person=_person,
        usedforsecurity=CryptoTruth("security", True),
    ),
)
for label, value in (("negative", -1), ("wide", 1 << 100)):
    crypto_probe(
        "blake-leaf-" + label,
        lambda value=value: hashlib.blake2b(
            _data, key=_key, leaf_size=CryptoIndex("leaf", value)
        ),
    )

_password = CryptoBuffer("password", b"password")
_kdf_salt = CryptoBuffer("salt", b"salt")


def mutate_kdf_inputs():
    _password.data[0] = ord("P")
    _kdf_salt.data[0] = ord("S")
    for item in (_password, _kdf_salt):
        try:
            item.data.append(0)
        except BufferError:
            _events.append((item.label, "pinned"))


crypto_probe(
    "pbkdf-conversions",
    lambda: hashlib.pbkdf2_hmac(
        "sha256",
        _password,
        _kdf_salt,
        CryptoIndex("rounds", 1, mutate_kdf_inputs),
        CryptoIndex("length", 16),
    ).hex(),
)
crypto_probe(
    "pbkdf-invalid-rounds",
    lambda: hashlib.pbkdf2_hmac(
        "sha256",
        _password,
        _kdf_salt,
        CryptoIndex("rounds", 0),
        CryptoIndex("length", 16),
    ),
)
crypto_probe(
    "pbkdf-intmax",
    lambda: hashlib.pbkdf2_hmac(
        "sha256", _password, _kdf_salt, (1 << 31), CryptoIndex("length", 16)
    ),
)
crypto_probe(
    "scrypt-conversion-order",
    lambda: hashlib.scrypt(
        _password,
        salt=_kdf_salt,
        n=3,
        r=1,
        p=1,
        maxmem=CryptoIndex("maxmem", -1),
        dklen=CryptoIndex("length", 16),
    ),
)
crypto_probe(
    "scrypt-index-version",
    lambda: hashlib.scrypt(
        _password,
        salt=_kdf_salt,
        n=CryptoIndex("n", 16),
        r=1,
        p=1,
        maxmem=CryptoIndex("maxmem", 1 << 20),
        dklen=CryptoIndex("length", 16),
    ).hex(),
)
crypto_probe(
    "scrypt-no-truncation",
    lambda: hashlib.scrypt(
        _password, salt=_kdf_salt, n=16, r=(1 << 32) + 1, p=1, dklen=16
    ),
)
print("crypto-exports-retired", end=" ")
for _input in (_data, _key, _salt, _person, _password, _kdf_salt):
    _input.data.append(0)
print(True)

_update_hash = hashlib.sha256(b"start")


def reenter_hash_update():
    _events.append(("hash", "copy", _update_hash.copy().hexdigest()))
    _update_hash.update(b"nested")


_update_buffer = CryptoBuffer("update", b"outer", reenter_hash_update)
crypto_probe(
    "hash-update-reentry",
    lambda: (_update_hash.update(_update_buffer), _update_hash.hexdigest()),
)
_update_buffer.data.append(0)

crypto_probe(
    "blake-c-int-bound",
    lambda: hashlib.blake2b(_data, digest_size=CryptoIndex("size", 1 << 31), key=_key),
)
crypto_probe(
    "pbkdf-c-long-bound",
    lambda: hashlib.pbkdf2_hmac(
        "sha256",
        _password,
        _kdf_salt,
        CryptoIndex("rounds", 1 << 100),
        CryptoIndex("length", 16),
    ),
)
crypto_probe(
    "scrypt-c-long-bound",
    lambda: hashlib.scrypt(
        _password,
        salt=_kdf_salt,
        n=16,
        r=1,
        p=1,
        maxmem=CryptoIndex("maxmem", 1 << 100),
        dklen=CryptoIndex("length", 16),
    ),
)
_xof = hashlib.shake_128(b"abc")
crypto_probe("shake-ssize-bound", lambda: _xof.digest(CryptoIndex("length", 1 << 100)))
crypto_probe("shake-negative", lambda: _xof.digest(CryptoIndex("length", -1)))

# A selected 3.14 builtin lookup calls lower only after the native attempt has
# retired its buffers. Older targets proceed directly to the original-name error.
class DigestLower(str):
    def lower(self):
        _events.append(("digest", "lower"))
        return "sha3_256"


class DigestLowerError(str):
    def lower(self):
        _events.append(("digest", "lower"))
        raise _marker


crypto_probe("hmac-builtin-new-order", lambda: hmac.new(
    b"key", CryptoBuffer("message", b"abc"), DigestLower("SHA3_256")
).hexdigest())
crypto_probe("hmac-builtin-single-order", lambda: hmac.digest(
    CryptoBuffer("key", b"key"), CryptoBuffer("message", b"abc"),
    DigestLower("SHA3_256"),
).hex())
crypto_probe("hmac-builtin-lower-error", lambda: hmac.digest(
    CryptoBuffer("key", b"key"), CryptoBuffer("message", b"abc"),
    DigestLowerError("SHA3_256"),
))


def name_export_error():
    raise _marker


for role, operation in (
    ("new", lambda: hashlib.new("sha512/224", CryptoBuffer("bad", b"x", name_export_error))),
    ("private-new", lambda: _hashlib.new("sha512/224", CryptoBuffer("bad", b"x", name_export_error))),
    ("pbkdf", lambda: hashlib.pbkdf2_hmac("sha512/224", CryptoBuffer("bad", b"x", name_export_error), b"salt", 1)),
):
    crypto_probe("unsupported-exporter-" + role, operation)

# C-module declarations remain owned even when their public aliases are rebound.
_original_unsupported = _hashlib.UnsupportedDigestmodError
class ReboundDigestError(ValueError):
    pass
_hashlib.UnsupportedDigestmodError = ReboundDigestError
for label, operation in (
    ("private-new", lambda: _hashlib.new("sha512/224")),
    ("pbkdf", lambda: hashlib.pbkdf2_hmac("sha512/224", b"p", b"s", 1)),
    ("private-hmac", lambda: _hashlib.hmac_new(b"k", b"v", "sha512/224")),
    ("public-hmac-dynamic-catch", lambda: hmac.new(b"k", b"v", "sha512/224")),
):
    try:
        operation()
    except Exception as error:
        show("rebound:" + label, (type(error) is _original_unsupported, str(error)))
_hashlib.UnsupportedDigestmodError = _original_unsupported

for invalid_name in (123, None):
    _events.clear()
    try:
        _hashlib.new(invalid_name, CryptoBuffer("data", b"abc"),
                     usedforsecurity=CryptoTruth("security", True))
    except Exception as error:
        show("private-name:" + repr(invalid_name), (type(error).__name__, str(error), _events))

if sys.version_info[:2] >= (3, 14):
    import _hmac as _builtin_hmac
    _original_unknown = _builtin_hmac.UnknownHashError
    _builtin_hmac.UnknownHashError = ReboundDigestError
    for label, operation in (
        ("new", lambda: _builtin_hmac.new(b"k", digestmod="unsupported")),
        ("single", lambda: _builtin_hmac.compute_digest(b"k", b"v", "unsupported")),
        ("public-dynamic-catch", lambda: hmac.new(b"k", b"v", "unsupported")),
    ):
        try:
            operation()
        except Exception as error:
            show("builtin-rebound:" + label, (type(error) is _original_unknown, str(error)))
    _builtin_hmac.UnknownHashError = _original_unknown

# Public name dispatch observes set/cache lookup, not a tuple equality filter.
class PublicName(str):
    def __hash__(self):
        _events.append(("name", "hash"))
        return str.__hash__(self)

    def __eq__(self, other):
        _events.append(("name", "equal"))
        raise _marker


crypto_probe("public-name-keyword", lambda: hashlib.new(PublicName("sha256"), bogus=1))
crypto_probe("public-name-security", lambda: hashlib.new(
    PublicName("sha256"), CryptoBuffer("data", b"abc"),
    usedforsecurity=CryptoTruth("security", True),
))
_saved_blake = hashlib.blake2b
hashlib.blake2b = lambda *args, **kwargs: (_ for _ in ()).throw(_marker)
try:
    show("captured-blake-constructor", hashlib.new("blake2b", b"abc", digest_size=17).hexdigest())
finally:
    hashlib.blake2b = _saved_blake

for _invalid_private_name in (123, None, "sha256\0"):
    try:
        _hashlib.new(_invalid_private_name)
    except Exception as error:
        show("private-name-literal:" + repr(_invalid_private_name), (type(error).__name__, str(error)))

crypto_probe("private-blake-security-before-data", lambda: _hashlib.new(
    "blake2b", CryptoBuffer("data", b"abc"),
    usedforsecurity=CryptoTruth("security", False),
).hexdigest())
