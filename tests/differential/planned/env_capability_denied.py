# MOLT_ENV: MOLT_CAPABILITIES=
"""Purpose: differential coverage for env capability denied."""

import os


try:
    _ = os.environ.get("PATH")
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
