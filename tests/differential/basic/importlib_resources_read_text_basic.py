# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources read text basic."""

import importlib.resources as resources

text = resources.read_text("tests.differential.planned", "res_pkg/data.txt")
print(text.strip())
