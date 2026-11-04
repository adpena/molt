"""Purpose: `_frozen_importlib_external` can be the first import-system module a
program imports. CPython has it loaded at startup; Molt's facade imports
`importlib.machinery`, whose package imports `importlib._bootstrap_external`
while `_frozen_importlib_external` is still initializing (HF-737)."""

import _frozen_importlib_external

print(_frozen_importlib_external.PathFinder.__name__)
print(_frozen_importlib_external.FileFinder.__module__)

import importlib._bootstrap_external as bootstrap_external  # noqa: E402

print(bootstrap_external.PathFinder is _frozen_importlib_external.PathFinder)
print(hasattr(bootstrap_external, "SourceFileLoader"))
from importlib._bootstrap_external import SourceFileLoader  # noqa: E402

print(SourceFileLoader is _frozen_importlib_external.SourceFileLoader)
