"""Purpose: differential coverage for subprocess.check_output kwarg parity."""

import subprocess
import sys


def _expect_error(label: str, **kwargs):
    try:
        subprocess.check_output([sys.executable, "-c", "print('ok')"], **kwargs)
        print(label, "missed")
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


def main():
    _expect_error("stdout_kw", stdout=subprocess.PIPE)
    _expect_error("check_kw", check=False)

    cmd = [
        sys.executable,
        "-c",
        "import sys; data = sys.stdin.buffer.read(); print(len(data))",
    ]
    print("input_none_bytes", subprocess.check_output(cmd, input=None).strip())
    print("input_none_text", subprocess.check_output(cmd, input=None, text=True).strip())


if __name__ == "__main__":
    main()
