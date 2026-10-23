# MOLT_ENV: MOLT_CAPABILITIES=
# MOLT_META: verified_subset_scope=capability_policy expect_fail=molt expect_fail_reason=requires_env_capability expect_molt_returncode=0 expect_molt_stdout_sha256=84f14278e10c65a093dcd62e8852e91ea7fabfca47859bef79a6a00093dc3835
"""Purpose: an empty capability grant denies an environment read.

CPython has no capabilities and prints ok; Molt must print PermissionError.
"""

import os


try:
    _ = os.environ.get("PATH")
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
