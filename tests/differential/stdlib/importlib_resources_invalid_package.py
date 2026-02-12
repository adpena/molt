# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources invalid package."""

import importlib.resources as resources

try:
    resources.files("not_a_package")
except Exception as exc:
    print(type(exc).__name__)
