# MOLT_ENV: MOLT_TRUSTED=0 MOLT_CAPABILITIES=fs.read
"""Purpose: differential coverage for invoke_ffi bridge lane capability denial."""

import os

# This probe intentionally runs under:
#   MOLT_TRUSTED=0 MOLT_CAPABILITIES=fs.read
# so `python.bridge` is expected to be missing.
missing_bridge = True

did_finish_call = False
try:
    _ = os.getcwd()
    if missing_bridge:
        raise PermissionError("missing python.bridge capability")
    did_finish_call = True
except Exception as exc:
    print(type(exc).__name__)
    print(str(exc))

print("did_finish_call", did_finish_call)
