"""Purpose: differential coverage for hmac basics."""

import hashlib
import hmac

print(hmac.compare_digest(b"abc", b"abc"))
print(hmac.compare_digest(b"abc", b"abd"))
print(hmac.digest(b"key", b"msg", "sha1").hex())
print(hmac.new(b"key", b"msg", hashlib.sha1).hexdigest())
