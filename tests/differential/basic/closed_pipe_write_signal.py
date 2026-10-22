"""Purpose: a write to a pipe whose reader closed follows the process's SIGPIPE disposition.

CPython ignores SIGPIPE at startup, so the write raises BrokenPipeError. A Molt
native program keeps the disposition it inherited, as a C program does, so the
default disposition ends the process with SIGPIPE. Every native code owner (the
AOT executable and the development JIT runner) must end the same way: the JIT
runner must not inherit Rust's start-up `SIG_IGN` (HF-371).

The pinned Molt outcome, for both tiers: Molt ends with SIGPIPE (exit -13) and
prints nothing. Any other ending, or two tiers that end differently, fails the
case.
"""
# MOLT_META: expect_fail=molt expect_fail_reason=sigpipe_default_disposition expect_molt_returncode=-13 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855

import os

reader, writer = os.pipe()
os.close(reader)
try:
    os.write(writer, b"x")
except BrokenPipeError:
    print("BrokenPipeError")
else:
    print("written")
