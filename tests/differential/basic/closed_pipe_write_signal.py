"""Purpose: a write to a pipe whose reader closed follows the process's SIGPIPE disposition.

CPython ignores SIGPIPE at startup, so the write raises BrokenPipeError. A Molt
native program keeps the disposition it inherited, as a C program does, so the
default disposition ends the process with SIGPIPE. Every native code owner (the
AOT executable and the development JIT runner) must end the same way: the JIT
runner must not inherit Rust's start-up `SIG_IGN` (HF-371).

The program prints a marker line before the write. The pinned Molt outcome,
for both tiers: Molt prints the marker, then ends with SIGPIPE (exit -13). A
crash before the write, any other ending, or two tiers that end differently,
fails the case.
"""
# MOLT_META: backends=jit,native platforms=linux,macos expect_fail=molt expect_fail_reason=sigpipe_default_disposition expect_molt_returncode=-13 expect_molt_stdout_sha256=8cd151b8f414b58aa1e1f3161e10b2d1770829220398ffaf4b8d37ed1d6bb5da

import os

reader, writer = os.pipe()
os.close(reader)
# Flushed now: a process that a signal ends loses buffered output.
print("writing to a pipe whose reader closed", flush=True)
try:
    os.write(writer, b"x")
except BrokenPipeError:
    print("BrokenPipeError")
else:
    print("written")
