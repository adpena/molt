# MOLT_ENV: PYTHONPATH=src:tests/differential/planned
"""Purpose: differential coverage for importlib resources is resource."""

import importlib.resources as resources


print(resources.is_resource("res_pkg", "data.txt"))
print(resources.is_resource("res_pkg", "missing.txt"))
