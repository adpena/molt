# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Purpose: differential coverage for import spec shape."""

import importlib


mod = importlib.import_module("math")
print(mod.__spec__ is not None)
print(mod.__spec__.origin is not None)

pkg = importlib.import_module("res_pkg")
print(pkg.__spec__ is not None)
print(pkg.__spec__.submodule_search_locations is not None)
