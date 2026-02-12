# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources missing subpackage."""

import importlib.resources as resources

try:
    resources.files("tests.differential.planned.missing")
except Exception as exc:
    print(type(exc).__name__)
