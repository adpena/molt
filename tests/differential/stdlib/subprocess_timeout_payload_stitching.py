"""Purpose: differential coverage for subprocess.TimeoutExpired payload stitching."""

import subprocess
import sys


def _timeout_cmd():
    return [
        sys.executable,
        "-c",
        (
            "import sys, time; "
            "sys.stdout.buffer.write(b'out-a|'); sys.stdout.flush(); "
            "sys.stderr.buffer.write(b'err-a|'); sys.stderr.flush(); "
            "time.sleep(0.05); "
            "sys.stdout.buffer.write(b'out-b|'); sys.stdout.flush(); "
            "sys.stderr.buffer.write(b'err-b|'); sys.stderr.flush(); "
            "time.sleep(5)"
        ),
    ]


def _as_bytes(payload):
    if isinstance(payload, (bytes, bytearray, memoryview)):
        return bytes(payload)
    return b""


def main():
    try:
        subprocess.run(
            _timeout_cmd(),
            capture_output=True,
            text=True,
            timeout=0.25,
            check=False,
        )
        print("timeout", "missed")
    except subprocess.TimeoutExpired as exc:
        out = _as_bytes(exc.output)
        err = _as_bytes(exc.stderr)
        print("types", type(exc.output).__name__, type(exc.stderr).__name__)
        print("payload", out.decode("utf-8"), err.decode("utf-8"))


if __name__ == "__main__":
    main()
