# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources subdir read binary."""

import importlib.resources as resources

blob = resources.read_binary("tests.differential.planned.res_pkg", "data.txt")
print(blob.startswith(b"hello"))
