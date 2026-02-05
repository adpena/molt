"""Purpose: differential coverage for hmac basics."""

import hashlib
import hmac

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
