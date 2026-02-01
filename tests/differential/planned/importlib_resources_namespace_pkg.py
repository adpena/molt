# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources namespace pkg."""

import importlib.resources as resources

try:
    resources.files("tests.differential.planned.ns_pkg")
except Exception as exc:
    print(type(exc).__name__)
