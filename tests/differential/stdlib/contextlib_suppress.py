"""Purpose: differential coverage for contextlib suppress."""

import contextlib


with contextlib.suppress(KeyError):
    raise KeyError("x")

print("ok")
