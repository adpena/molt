# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources path errors."""

import importlib.resources as resources

try:
    resources.files("tests.differential.planned").joinpath("missing.txt").read_text()
except Exception as exc:
    print(type(exc).__name__)
