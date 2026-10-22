"""Purpose: a write to a pipe whose reader closed follows the process's SIGPIPE disposition.

CPython ignores SIGPIPE at startup, so the write raises BrokenPipeError. A Molt
native program keeps the disposition it inherited, as a C program does, so the
default disposition ends the process with SIGPIPE. Every native code owner (the
AOT executable and the development JIT runner) must end the same way: the JIT
runner must not inherit Rust's start-up `SIG_IGN` (HF-371).
"""
# MOLT_META: expect_fail=molt expect_fail_reason=sigpipe_default_disposition

import os

reader, writer = os.pipe()
os.close(reader)
try:
    os.write(writer, b"x")
except BrokenPipeError:
    print("BrokenPipeError")
else:
    print("written")
