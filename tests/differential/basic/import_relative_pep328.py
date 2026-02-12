# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
"""Purpose: differential coverage for PEP 328 relative imports."""

import importlib


mod = importlib.import_module("rel_pkg.sub.mod_c")
print(mod.VALUE_C)
