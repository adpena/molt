# MOLT_META: backends=jit,llvm,native host_toolchains=posix:posix-sh
# MOLT_ENV: MOLT_CAPABILITIES=process.exec
"""Purpose: differential coverage for os.system shell execution.

POSIX CPython converts `command` with PyUnicode_FSConverter: str, bytes and
os.PathLike are accepted, anything else raises the os.fspath TypeError, and an
embedded NUL raises ValueError. The result is the raw wait status. WASI has no
process spawning (CPython's WASI build has no os.system), so the WASM and Luau
backends are outside this program's matrix.
"""

import os


class CommandPath:
    def __fspath__(self):
        return "exit 4"


print("empty", os.system(""))
print("exit7", os.system("exit 7"))
print("bytes", os.system(b"exit 3"))
print("pathlike", os.system(CommandPath()))
print("keyword", os.system(command="exit 5"))
print("signal", os.system("kill -9 $$"))

for value in (None, 1, 2.5, bytearray(b"exit 1"), ["exit 1"]):
    try:
        os.system(value)
    except Exception as exc:
        print("type", type(value).__name__, type(exc).__name__, str(exc))

for value in ("echo\x00hi", b"echo\x00hi"):
    try:
        os.system(value)
    except Exception as exc:
        print("nul", type(value).__name__, type(exc).__name__, str(exc))
