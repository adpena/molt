# MOLT_ENV: PYTHONPATH=src:tests/differential/planned
"""Purpose: differential coverage for PEP 451 ModuleSpec fields."""

import importlib.util


spec = importlib.util.find_spec("math")
print(spec.name, spec.loader is not None)
print(spec.origin is not None, spec.has_location)
print(spec.cached is not None)

pkg_spec = importlib.util.find_spec("res_pkg")
print(pkg_spec.name, pkg_spec.submodule_search_locations is not None)
print(pkg_spec.parent)
