# MOLT_ENV: MOLT_CAPABILITIES=
"""Purpose: differential coverage for fs capability denied."""

from pathlib import Path


path = Path("molt_denied.txt")
try:
    path.write_text("x")
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
