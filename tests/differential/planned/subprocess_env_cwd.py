"""Purpose: differential coverage for subprocess env and cwd."""

import os
import subprocess
import sys
import tempfile


def main():
    with tempfile.TemporaryDirectory() as tmp:
        env = os.environ.copy()
        env["MOLT_SUBPROC"] = "ok"
        result = subprocess.run(
            [sys.executable, "-c", "import os; print(os.getcwd()); print(os.getenv('MOLT_SUBPROC'))"],
            cwd=tmp,
            env=env,
            capture_output=True,
            text=True,
        )
        lines = result.stdout.strip().splitlines()
        print("cwd", os.path.abspath(lines[0]))
        print("env", lines[1])


if __name__ == "__main__":
    main()
