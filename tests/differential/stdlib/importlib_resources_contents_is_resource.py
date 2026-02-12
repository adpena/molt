# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources contents is resource."""

import importlib.resources as resources

names = list(resources.contents("tests.differential.planned"))
print("res_pkg" in names)
print(resources.is_resource("tests.differential.planned", "res_pkg/data.txt"))
