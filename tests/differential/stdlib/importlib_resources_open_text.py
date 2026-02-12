# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Purpose: differential coverage for importlib resources open text."""

import importlib.resources as resources


with resources.open_text("res_pkg", "data.txt") as handle:
    print(handle.read().strip())
