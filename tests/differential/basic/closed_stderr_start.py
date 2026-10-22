"""Purpose: a program started with stderr closed runs, as a C program does.

A shell's `2>&-` starts a program with file descriptor 2 closed. CPython runs
with sys.stderr set to None, and a native AOT executable starts with fd 2 closed. The
development JIT runner captures compiler output on fd 2 while it compiles; it
must compile with nothing to capture and start the program with fd 2 still
closed, not refuse to start (HF-371).

Both native code owners then differ from CPython the same way: the runtime
opens /dev/null at start, and with fd 2 closed that descriptor becomes fd 2,
so the child sees stderr open. The twins must agree on that until the runtime
keeps a closed fd 2 closed (HF-455).

The pinned Molt outcome, for both tiers: Molt exits 0 and prints "rc 0" and
"child ran, stderr open". Any other ending, or two tiers that end differently,
fails the case.
"""
# MOLT_ENV: MOLT_CAPABILITIES=process.exec,env.read,fs.read
# MOLT_META: expect_fail=molt expect_fail_reason=closed_stderr_reused_by_runtime expect_molt_returncode=0 expect_molt_stdout_sha256=e15a6f107fc2dec693de3efbf1918a921665d2c81336012ebc9974df56c7555d

import os
import subprocess
import sys


def child() -> None:
    try:
        os.fstat(2)
        state = "open"
    except OSError:
        state = "closed"
    print("child ran, stderr", state)


def main() -> None:
    if sys.argv[1:2] == ["child"]:
        child()
        return
    program = sys.argv[0]
    # CPython runs the script through its interpreter; a compiled program is
    # its own executable.
    command = [sys.executable, program] if program.endswith(".py") else [program]
    result = subprocess.run(
        ["/bin/sh", "-c", 'exec "$@" 2>&-', "sh", *command, "child"],
        capture_output=True,
        text=True,
    )
    print("rc", result.returncode)
    print(result.stdout, end="")


main()
