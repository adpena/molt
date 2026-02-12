# MOLT_ENV: MOLT_TRUSTED=0 MOLT_CAPABILITIES=python.bridge,fs.read
"""Purpose: exercise invoke_ffi bridge lane with explicit capability gating."""

import os

cwd = os.getcwd()
print(bool(cwd), cwd.count("/"))
