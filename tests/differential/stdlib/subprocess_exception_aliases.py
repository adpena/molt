"""Purpose: differential coverage for subprocess exception stdout aliases."""

import subprocess


def main():
    timeout_exc = subprocess.TimeoutExpired("cmd", 1.0, output=b"out", stderr=b"err")
    print("timeout_alias", timeout_exc.output == timeout_exc.stdout, timeout_exc.stderr)
    timeout_exc.stdout = b"updated"
    print("timeout_set", timeout_exc.output, timeout_exc.stdout)

    called_exc = subprocess.CalledProcessError(
        3, "cmd", output=b"cout", stderr=b"cerr"
    )
    print("called_alias", called_exc.output == called_exc.stdout, called_exc.stderr)
    called_exc.stdout = b"updated"
    print("called_set", called_exc.output, called_exc.stdout)


if __name__ == "__main__":
    main()
