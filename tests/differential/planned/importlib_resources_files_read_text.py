# MOLT_ENV: PYTHONPATH=src:tests/differential/planned
"""Purpose: differential coverage for importlib resources files read text."""

import importlib.resources as resources


text = resources.files("res_pkg").joinpath("data.txt").read_text().strip()
print(text)
