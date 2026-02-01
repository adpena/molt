# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources errors basic."""

import importlib.resources as resources

try:
    resources.read_text("tests.differential.planned", "missing.txt")
except Exception as exc:
    print(type(exc).__name__)
