"""Purpose: validate intrinsic-backed importlib.util extension-module find_spec."""

import importlib.util
import os
import sys
import tempfile


root = tempfile.mkdtemp(prefix="molt_ext_find_spec_")
ext_path = os.path.join(root, "extdemo.so")
with open(ext_path, "wb") as handle:
    handle.write(b"")

orig_path = list(sys.path)
orig_modules = {name: sys.modules.get(name) for name in ("extdemo",)}
spec_cache = getattr(importlib.util, "_SPEC_CACHE", None)
if isinstance(spec_cache, dict):
    spec_cache.clear()

try:
    sys.path[:] = [root]
    spec = importlib.util.find_spec("extdemo")
finally:
    sys.path[:] = orig_path
    for name, previous in orig_modules.items():
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous
    if isinstance(spec_cache, dict):
        spec_cache.clear()

origin = os.path.normpath(spec.origin) if spec and spec.origin else ""
print(spec is not None)
print(bool(origin) and origin.endswith(os.path.normpath("extdemo.so")))
print(spec is not None and spec.loader is not None)
print(spec is not None and bool(getattr(spec, "has_location", False)))
