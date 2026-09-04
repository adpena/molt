"""Purpose: differential coverage for invoke_ffi on non-allowlisted os call."""
# MOLT_META: verified_subset_scope=capability_policy expect_fail=molt expect_fail_reason=requires_ffi

import os

cwd = os.getcwd()
print(type(cwd).__name__)
print(cwd.startswith("/"))
print(len(cwd) > 0)
