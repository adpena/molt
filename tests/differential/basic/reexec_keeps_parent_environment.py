"""Purpose: a program started again by its own path sees the environment its parent passed.

A compiled program's argv[0] is its executable; subprocess.run and spawn-mode
multiprocessing start it again by that path. Nothing may stand between the
parent and the program: a shell would reset a stale PWD, drop OLDPWD and set
SHLVL. Every native code owner (the AOT executable and the development JIT
image, whose interpreter is the molt-jit runner) must pass the parent's
environment and arguments through unchanged (HF-371).
"""
# MOLT_ENV: MOLT_CAPABILITIES=process.exec,env.read,fs.read

import os
import subprocess
import sys


def child() -> None:
    print("PWD", os.environ.get("PWD"))
    print("OLDPWD", os.environ.get("OLDPWD"))
    print("SHLVL", os.environ.get("SHLVL"))
    print("args", sys.argv[2:])


def main() -> None:
    if sys.argv[1:2] == ["child"]:
        child()
        return
    program = sys.argv[0]
    # CPython runs the script through its interpreter; a compiled program is
    # its own executable.
    command = [sys.executable, program] if program.endswith(".py") else [program]
    environment = {name: value for name, value in os.environ.items() if name != "SHLVL"}
    environment["PWD"] = "/nonexistent/stale/dir"
    environment["OLDPWD"] = "/previous/dir"
    result = subprocess.run(
        [*command, "child", "two words", "$HOME"],
        env=environment,
        capture_output=True,
        text=True,
    )
    print("rc", result.returncode)
    print(result.stdout, end="")


main()
