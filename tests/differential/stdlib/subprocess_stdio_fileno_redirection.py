"""Purpose: differential coverage for subprocess stdio redirection via fileno()."""

import subprocess
import sys
import tempfile
from pathlib import Path


class FDWrapper:
    def __init__(self, fd: int):
        self._fd = fd

    def fileno(self) -> int:
        return self._fd


def main():
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        out_path = root / "stdout.txt"
        err_path = root / "stderr.txt"

        with out_path.open("wb") as out_file:
            proc = subprocess.Popen(  # noqa: S603
                [sys.executable, "-c", "print('fd-stdout')"],
                stdout=out_file,
            )
            print("out_rc", proc.wait())

        with err_path.open("wb") as err_file:
            proc = subprocess.Popen(  # noqa: S603
                [sys.executable, "-c", "import sys; sys.stderr.write('fd-stderr\\n')"],
                stderr=FDWrapper(err_file.fileno()),
            )
            print("err_rc", proc.wait())

        print("out_data", out_path.read_text(encoding="utf-8").strip())
        print("err_data", err_path.read_text(encoding="utf-8").strip())


if __name__ == "__main__":
    main()
