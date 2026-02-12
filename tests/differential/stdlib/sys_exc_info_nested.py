"""Purpose: differential coverage for sys exc info nested."""

import sys


try:
    try:
        raise ValueError("inner")
    except Exception:
        raise RuntimeError("outer")
except Exception:
    info = sys.exc_info()
    print(info[0].__name__)
