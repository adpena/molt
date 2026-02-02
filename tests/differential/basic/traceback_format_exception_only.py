"""Purpose: validate traceback.format_exception_only output shape."""

import traceback

try:
    raise ValueError("bad")
except Exception as exc:
    line = traceback.format_exception_only(type(exc), exc)[0].strip()
    print(line)
