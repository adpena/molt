"""Purpose: differential coverage for re flags, groups, and error cases."""

import re


def show(pattern, flags=0):
    try:
        compiled = re.compile(pattern, flags)
        print("ok", compiled.pattern, compiled.flags)
    except Exception as exc:
        print("err", type(exc).__name__)


show(r"(?P<name>ab)c")
show(r"[a-z]+", re.IGNORECASE | re.MULTILINE)
show(r"(?s)dot.*")
show(r"(?x)a # comment\n b")
show(r"(")
