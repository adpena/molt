"""Purpose: registering builtin callables keeps their references balanced.

The register intrinsic once returned its borrowed callable as an owned result,
so every registration released one reference to a builtin: three registrations
of `print` freed it before the callbacks ran, and runtime teardown aborted.
"""

import atexit

for _ in range(3):
    atexit.register(print, "print-callback")
atexit.register(len, "abc")
print("register returns func", atexit.register(print, "returned") is print)
atexit.unregister(len)
print("callbacks", atexit._ncallbacks())
print("print still works")
