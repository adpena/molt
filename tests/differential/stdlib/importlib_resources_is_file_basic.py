# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources is file basic."""

import importlib.resources as resources

p = resources.files("tests.differential.planned").joinpath("res_pkg/data.txt")
print(p.is_file())
