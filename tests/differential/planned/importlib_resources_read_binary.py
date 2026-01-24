# MOLT_ENV: PYTHONPATH=src:tests/differential/planned
"""Purpose: differential coverage for importlib resources read binary."""

import importlib.resources as resources


payload = resources.read_binary("res_pkg", "data.txt")
print(payload.decode().strip())
