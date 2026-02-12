# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources files error basic."""

import importlib.resources as resources

try:
    resources.files("missing.package")
except Exception as exc:
    print(type(exc).__name__)
