"""Purpose: differential coverage for importlib basic."""

import importlib
import importlib.util


spec = importlib.util.find_spec("math")
print(spec is not None)
mod = importlib.import_module("math")
print(hasattr(mod, "sqrt"))
