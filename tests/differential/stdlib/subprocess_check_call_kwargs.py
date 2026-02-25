"""Purpose: differential coverage for subprocess.check_call kwarg parity."""

import subprocess
import sys


def _expect_type_error(label: str, **kwargs):
    try:
        subprocess.check_call([sys.executable, "-c", "print('ok')"], **kwargs)  # noqa: S603
        print(label, "missed")
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


def main():
    _expect_type_error("input_kw", input=b"x")
    _expect_type_error("capture_output_kw", capture_output=True)
    _expect_type_error("check_kw", check=False)
    print("ok_rc", subprocess.check_call([sys.executable, "-c", "import sys; sys.exit(0)"]))  # noqa: S603


if __name__ == "__main__":
    main()
