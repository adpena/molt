# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources subdir traversal."""

import importlib.resources as resources

root = resources.files("tests.differential.planned").joinpath("res_pkg")
print(root.is_dir())
print(any(p.name == "data.txt" for p in root.iterdir()))
