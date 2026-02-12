# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources as file basic."""

import importlib.resources as resources

with resources.as_file(resources.files("tests.differential.planned").joinpath("res_pkg/data.txt")) as path:
    print(path.exists())
