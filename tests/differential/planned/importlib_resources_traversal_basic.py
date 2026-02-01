# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources traversal basic."""

import importlib.resources as resources

root = resources.files("tests.differential.planned")
print(root.is_dir())
print(any(p.name == "res_pkg" for p in root.iterdir()))
