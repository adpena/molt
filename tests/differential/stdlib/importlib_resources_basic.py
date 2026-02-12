# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Purpose: differential coverage for importlib resources basic."""

import importlib.resources as resources


with resources.files("res_pkg").joinpath("data.txt").open("r") as handle:
    text = handle.read().strip()
print(text)
