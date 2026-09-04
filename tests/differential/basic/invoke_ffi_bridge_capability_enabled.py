# MOLT_ENV: MOLT_CAPABILITY_TIER=none MOLT_CAPABILITIES=python.bridge,fs.read
# MOLT_META: verified_subset_scope=capability_policy expect_fail=molt expect_fail_reason=requires_ffi
"""Purpose: exercise invoke_ffi bridge lane with explicit capability gating."""

import os

cwd = os.getcwd()
print(bool(cwd), cwd.count("/"))
