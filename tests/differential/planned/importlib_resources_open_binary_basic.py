# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources open binary basic."""

import importlib.resources as resources

with resources.open_binary("tests.differential.planned", "res_pkg/data.txt") as handle:
    print(handle.read(5))
