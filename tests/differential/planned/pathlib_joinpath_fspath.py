"""Purpose: differential coverage for pathlib joinpath + os.fspath."""

from pathlib import Path
import os


def main():
    base = Path("root")
    joined = base.joinpath("a", Path("b"))
    print("join", joined.as_posix())
    print("fspath", os.fspath(joined))
    print("fspath_type", type(os.fspath(joined)).__name__)


if __name__ == "__main__":
    main()
