"""Purpose: verify relative importlib imports resolve correctly inside a package."""

import importlib


pkg = importlib.import_module("importlib")
leaf = importlib.import_module(".util", pkg.__name__)
absolute = importlib.import_module("importlib.util")

print(pkg.__name__)
print(leaf.__name__.split(".")[-1])
print(absolute.__name__.split(".")[-1])
print(leaf.__spec__ is not None)
print(absolute.__spec__ is not None)
