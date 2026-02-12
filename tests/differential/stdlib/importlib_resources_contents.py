# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Purpose: differential coverage for importlib resources contents."""

import importlib.resources as resources


items = sorted(resources.contents("res_pkg"))
print(items)
