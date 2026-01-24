# MOLT_ENV: PYTHONPATH=src:tests/differential/planned
"""Purpose: differential coverage for import sys modules override."""

import importlib
import sys
import types


mod = types.ModuleType("molt_temp_mod")
mod.VALUE = 42
sys.modules["molt_temp_mod"] = mod

molt_temp_mod = importlib.import_module("molt_temp_mod")
print(molt_temp_mod.VALUE)
