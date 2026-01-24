# MOLT_ENV: PYTHONPATH=src:tests/differential/planned
"""Purpose: differential coverage for importlib module reload."""

import importlib
import res_pkg


print(res_pkg.VALUE)
res_pkg.VALUE = "changed"
res_pkg = importlib.reload(res_pkg)
print(res_pkg.VALUE)
