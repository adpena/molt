"""Purpose: differential coverage for subprocess args/env bytes+pathlike coercion."""

import os
import subprocess
import sys


class FspathValue:
    def __init__(self, value):
        self._value = value

    def __fspath__(self):
        return self._value


def main():
    env = os.environ.copy()
    env[FspathValue("MOLT_SUBPROCESS_STR_ENV")] = FspathValue("env-pathlike")
    env[FspathValue(b"MOLT_SUBPROCESS_BYTES_ENV")] = FspathValue(b"env-bytes")

    cmd = [
        FspathValue(sys.executable),
        "-c",
        (
            "import os, sys; "
            "print('argv1', sys.argv[1]); "
            "print('argv2', sys.argv[2]); "
            "print('env_str', os.getenv('MOLT_SUBPROCESS_STR_ENV')); "
            "print('env_bytes', os.getenv('MOLT_SUBPROCESS_BYTES_ENV'))"
        ),
        FspathValue("arg-pathlike"),
        FspathValue(b"arg-bytes"),
    ]

    result = subprocess.run(cmd, env=env, capture_output=True, text=True, check=True)
    print(result.stdout.strip())


if __name__ == "__main__":
    main()
