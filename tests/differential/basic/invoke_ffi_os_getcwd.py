"""Purpose: differential coverage for invoke_ffi on non-allowlisted os call."""
# MOLT_META: expect_fail=molt expect_fail_reason=requires_ffi

import os

cwd = os.getcwd()
print(type(cwd).__name__)
print(cwd.startswith("/"))
print(len(cwd) > 0)
