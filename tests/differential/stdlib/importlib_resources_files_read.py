# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources files read."""

import importlib.resources as resources

files = resources.files("tests.differential.planned")
print(files.is_dir())
