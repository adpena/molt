"""Purpose: differential coverage for contextlib nullcontext."""

import contextlib


with contextlib.nullcontext(5) as val:
    print(val)
