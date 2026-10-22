"""Purpose: a write past the file size limit follows the process's SIGXFSZ disposition.

CPython ignores SIGXFSZ at startup, so a write past the limit fails with EFBIG
and raises OSError. A Molt native program keeps the disposition it inherited,
as a C program does, so the default disposition ends it with SIGXFSZ (HF-441).
Every native code owner must end the same way. The development JIT runner
ignores SIGXFSZ only while it compiles; it must give the program the
disposition it inherited (HF-371).

The program starts itself again under `ulimit -f 0` with SIGXFSZ at its default,
and the child prints a marker line before its write. The pinned Molt outcome,
for both tiers: the child prints the marker, then ends with SIGXFSZ (exit -25).
Any other ending, or two tiers that end differently, fails the case.
"""
# MOLT_ENV: MOLT_CAPABILITIES=process.exec,env.read,fs.read,fs.write
# MOLT_META: backends=jit,native platforms=linux,macos expect_fail=molt expect_fail_reason=sigxfsz_default_disposition expect_molt_returncode=0 expect_molt_stdout_sha256=266e4e89aa18ba0df9b0a5bde624661de3c5b124768e3cb4527223344b95da93

import os
import subprocess
import sys
import tempfile


def child() -> None:
    # Flushed now: stdout is a pipe, and a process that a signal ends loses
    # buffered output.
    print("start", flush=True)
    try:
        with open("out.bin", "wb") as handle:
            handle.write(b"x" * 10)
        print("written")
    except OSError as error:
        print("OSError", error.errno)


def main() -> None:
    if sys.argv[1:2] == ["child"]:
        child()
        return
    program = os.path.abspath(sys.argv[0])
    # CPython runs the script through its interpreter; a compiled program is
    # its own executable.
    command = [sys.executable, program] if program.endswith(".py") else [program]
    with tempfile.TemporaryDirectory() as directory:
        result = subprocess.run(
            ["/bin/sh", "-c", 'ulimit -f 0; exec "$@"', "sh", *command, "child"],
            cwd=directory,
            capture_output=True,
            text=True,
        )
    print("child rc", result.returncode)
    print(result.stdout, end="")


main()
