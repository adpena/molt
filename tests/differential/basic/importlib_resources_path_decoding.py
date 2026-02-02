# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: differential coverage for importlib resources path decoding."""

import importlib.resources as resources

with resources.open_text("tests.differential.planned", "res_pkg/data.txt", encoding="utf-8") as handle:
    print(handle.read().strip())
