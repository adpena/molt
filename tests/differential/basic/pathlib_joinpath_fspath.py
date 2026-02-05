"""Purpose: differential coverage for pathlib joinpath + os.fspath."""

from pathlib import Path
import os


def main():
    base = Path("root")
    joined = base.joinpath("a", Path("b"))
    print("join", joined.as_posix())
    print("fspath", os.fspath(joined))
    print("fspath_type", type(os.fspath(joined)).__name__)
    class PathLikeStr:
        def __fspath__(self):
            return "root/like"

    class PathLikeBytes:
        def __fspath__(self):
            return b"root/bytes"

    print("pathlike", str(Path(PathLikeStr())))
    try:
        Path(PathLikeBytes())
        print("pathlike-bytes-ok")
    except TypeError:
        print("pathlike-bytes-typeerror")


if __name__ == "__main__":
    main()
