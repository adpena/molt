# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
"""Purpose: __main__ relative import with __package__ override."""

__package__ = "rel_main_pkg"
from .helper import VALUE

print("value", VALUE)
