"""Purpose: differential coverage for contextlib suppress multiple."""

import contextlib


with contextlib.suppress(KeyError, ValueError):
    raise ValueError("boom")

print("ok")
