"""Purpose: validate traceback caret shaping for assertion frames."""

import traceback


def _boom() -> None:
    value = False
    assert value


caret_lines = []
source_lines = []
try:
    _boom()
except BaseException as exc:
    formatted = traceback.format_exception(type(exc), exc, exc.__traceback__)
    caret_lines = [line for line in formatted if "^" in line]
    source_lines = [line for line in formatted if "assert value" in line]

print(bool(caret_lines))
print(bool(source_lines))
print(caret_lines[0].index("^") >= 8 if caret_lines else False)
