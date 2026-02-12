# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Purpose: differential coverage for importlib resources iterdir."""

import importlib.resources as resources


files = sorted(p.name for p in resources.files("res_pkg").iterdir())
print(files)
