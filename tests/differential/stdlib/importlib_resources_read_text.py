# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Purpose: differential coverage for importlib resources read text."""

import importlib.resources as resources


print(resources.read_text("res_pkg", "data.txt").strip())
