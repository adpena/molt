# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources read text errors."""

import importlib.resources as resources

try:
    resources.read_text("tests.differential.planned", "res_pkg/data.txt", encoding="utf-8", errors="strict")
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
