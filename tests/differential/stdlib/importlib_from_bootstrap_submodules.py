import importlib

from importlib import _bootstrap, _bootstrap_external

direct_bootstrap = importlib.import_module("importlib._bootstrap")
direct_external = importlib.import_module("importlib._bootstrap_external")

print(_bootstrap is direct_bootstrap)
print(_bootstrap_external is direct_external)
print(_bootstrap.__name__)
print(_bootstrap_external.__name__)
print(hasattr(_bootstrap, "ModuleSpec"))
print(hasattr(_bootstrap_external, "PathFinder"))
