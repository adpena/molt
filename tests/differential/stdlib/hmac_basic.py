# MOLT_META: stderr=exact
"""Purpose: differential coverage for hmac basics."""

import hashlib
import hmac
import _hashlib

print(hmac.compare_digest(b"abc", b"abc"))
print(hmac.compare_digest(b"abc", b"abd"))
print(hmac.digest(b"key", b"msg", "sha1").hex())
print(hmac.new(b"key", b"msg", hashlib.sha1).hexdigest())

try:
    hmac.new(b"key", b"msg")
except TypeError as exc:
    print(type(exc).__name__, str(exc))

h = hmac.new(b"key", b"msg", digestmod="sha256")
print(h.name)
print(h.digest_size, h.block_size)
h2 = h.copy()
h.update(b"more")
print(h2.hexdigest())
print(h.hexdigest())

h3 = hmac.HMAC(b"key", b"msg", digestmod=hashlib.sha256)
print(h3.hexdigest())

# Native New and SingleShot have different Clinic acquisition boundaries.

_events = []
_marker = RuntimeError("hmac callback marker")


class HmacBuffer:
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


class DigestLookup:
    def __hash__(self):
        _events.append(("digest", "hash"))
        raise _marker

    def __repr__(self):
        return "DigestLookup"


def hmac_probe(label, action):
    _events.clear()
    try:
        result = action()
        print(label, result)
    except Exception as error:
        print(label, type(error).__name__, error is _marker)
    print(label, "events", _events)


_key = HmacBuffer("key", b"key")
_msg = HmacBuffer("msg", b"msg")
hmac_probe("hmac-public-key-admission", lambda: hmac.new(_key, _msg, DigestLookup()))
hmac_probe(
    "hmac-native-new", lambda: _hashlib.hmac_new(_key, _msg, "sha256").hexdigest()
)
hmac_probe(
    "hmac-native-digest", lambda: _hashlib.hmac_digest(_key, _msg, "sha256").hex()
)
hmac_probe("hmac-public-digest", lambda: hmac.digest(_key, _msg, "sha256").hex())
hmac_probe("hmac-new-lookup", lambda: _hashlib.hmac_new(_key, _msg, DigestLookup()))
hmac_probe(
    "hmac-digest-lookup", lambda: _hashlib.hmac_digest(_key, _msg, DigestLookup())
)
hmac_probe("hmac-new-missing", lambda: _hashlib.hmac_new(_key, _msg))
hmac_probe("hmac-new-none", lambda: _hashlib.hmac_new(_key, _msg, None))
hmac_probe("hmac-unsupported-builtin", lambda: hmac.digest(_key, _msg, len))


def mutate_key_from_msg():
    _key.data[0] = ord("K")
    try:
        _key.data.append(0)
    except BufferError:
        _events.append(("key", "pinned"))


_msg.action = mutate_key_from_msg
_key.data[:] = b"key"
hmac_probe(
    "hmac-new-key-read-order",
    lambda: _hashlib.hmac_new(_key, _msg, "sha256").hexdigest(),
)
_key.data[:] = b"key"
hmac_probe(
    "hmac-digest-key-read-order",
    lambda: _hashlib.hmac_digest(_key, _msg, "sha256").hex(),
)
_msg.action = None
_key.data.append(0)
_msg.data.append(0)
print("hmac-exports-retired", True)


# Custom digest factories retain the public two-object protocol and order.
class LoggedHash:
    def __init__(self, number, data=b""):
        self.number = number
        self._hash = hashlib.sha256(data)
        self.digest_size = self._hash.digest_size
        self.block_size = self._hash.block_size
        self.name = self._hash.name

    def update(self, data):
        _events.append(("hash", self.number, "update", len(data)))
        self._hash.update(data)

    def digest(self):
        _events.append(("hash", self.number, "digest"))
        return self._hash.digest()

    def hexdigest(self):
        _events.append(("hash", self.number, "hexdigest"))
        return self._hash.hexdigest()

    def copy(self):
        _events.append(("hash", self.number, "copy"))
        result = LoggedHash(self.number)
        result._hash = self._hash.copy()
        return result


_factory_count = 0


def logged_factory(data=b""):
    global _factory_count
    _factory_count += 1
    _events.append(("factory", _factory_count, len(data)))
    return LoggedHash(_factory_count, data)


hmac_probe(
    "hmac-custom-new", lambda: hmac.new(b"key", b"msg", logged_factory).hexdigest()
)
_factory_count = 0
hmac_probe(
    "hmac-custom-digest", lambda: hmac.digest(b"key", b"msg", logged_factory).hex()
)
print("hmac-constructor-map", _hashlib._constructors[_hashlib.openssl_sha256])

_update_hmac = hmac.new(b"key", b"start", "sha256")


def reenter_hmac_update():
    _events.append(("hmac", "copy", _update_hmac.copy().hexdigest()))
    _update_hmac.update(b"nested")


_update_buffer = HmacBuffer("update", b"outer", reenter_hmac_update)
hmac_probe(
    "hmac-update-reentry",
    lambda: (_update_hmac.update(_update_buffer), _update_hmac.hexdigest()),
)
_update_buffer.data.append(0)
