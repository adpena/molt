"""Purpose: a program starts whatever TMPDIR, file size limit and current directory its caller gives.

A native AOT executable starts without touching the file system, and CPython
starts in each case below. The development JIT runner compiles the program at
start (HF-371), so it must not need a usable TMPDIR, must not write past a file
size limit of zero (`ulimit -f 0`, with SIGXFSZ at its default disposition),
and must not leave files in the current directory, even with MOLT_CACHE unset.
Each child prints what its current directory holds: nothing.
"""
# MOLT_ENV: MOLT_CAPABILITIES=process.exec,env.read,fs.read,fs.write
# MOLT_META: backends=jit,native platforms=linux,macos

import os
import subprocess
import sys
import tempfile


def child() -> None:
    print("child sees", sorted(os.listdir(".")))


def run(label: str, command: list[str], directory: str, environment: dict) -> None:
    result = subprocess.run(
        command, cwd=directory, env=environment, capture_output=True, text=True
    )
    print(label, "rc", result.returncode)
    print(result.stdout, end="")


def main() -> None:
    if sys.argv[1:2] == ["child"]:
        child()
        return
    program = os.path.abspath(sys.argv[0])
    # CPython runs the script through its interpreter; a compiled program is
    # its own executable.
    command = [sys.executable, program] if program.endswith(".py") else [program]
    environment = {
        name: value for name, value in os.environ.items() if name != "MOLT_CACHE"
    }
    with tempfile.TemporaryDirectory() as directory:
        unusable = dict(environment, TMPDIR="/nonexistent/molt-start-tmp")
        run("unusable TMPDIR", [*command, "child"], directory, unusable)
        limited = ["/bin/sh", "-c", 'ulimit -f 0; exec "$@"', "sh", *command, "child"]
        run("file size limit 0", limited, directory, environment)
        print("left behind", sorted(os.listdir(directory)))


main()
