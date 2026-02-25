"""Purpose: differential coverage for subprocess.TimeoutExpired payload types."""

import subprocess
import sys


def main():
    cmd = [
        sys.executable,
        "-c",
        (
            "import sys, time; "
            "sys.stdout.write('tout'); sys.stdout.flush(); "
            "sys.stderr.write('terr'); sys.stderr.flush(); "
            "time.sleep(2)"
        ),
    ]
    try:
        subprocess.run(cmd, capture_output=True, text=True, timeout=0.1, check=False)
        print("timeout", "missed")
    except subprocess.TimeoutExpired as exc:
        print("types", type(exc.output).__name__, type(exc.stderr).__name__)
        out = exc.output if isinstance(exc.output, (bytes, bytearray)) else b""
        err = exc.stderr if isinstance(exc.stderr, (bytes, bytearray)) else b""
        print("payload", out.decode("utf-8"), err.decode("utf-8"))


if __name__ == "__main__":
    main()
