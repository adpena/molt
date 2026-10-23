# MOLT_ENV: MOLT_CAPABILITIES=
# MOLT_META: verified_subset_scope=capability_policy expect_fail=molt expect_fail_reason=requires_fs_capability expect_molt_returncode=0 expect_molt_stdout_sha256=84f14278e10c65a093dcd62e8852e91ea7fabfca47859bef79a6a00093dc3835
"""Purpose: an empty capability grant denies a file write.

CPython has no capabilities: it writes, removes the file and prints ok. Molt
must print PermissionError and write nothing.
"""

from pathlib import Path


path = Path("molt_denied.txt")
try:
    path.write_text("x")
    path.unlink()
    print("ok")
except Exception as exc:
    print(type(exc).__name__)
