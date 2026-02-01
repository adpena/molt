"""Behavior: SimpleCookie emits Max-Age/Expires attributes for morsels.
Why: HTTP cookie serialization must match CPython for web compatibility.
Pitfalls: Attribute ordering and formatting can differ across implementations.
"""

from http import cookies

c = cookies.SimpleCookie()
c["a"] = "b"
c["a"]["max-age"] = 60
c["a"]["expires"] = "Wed, 21 Oct 2015 07:28:00 GMT"
print(c.output(sep="; "))
