"""Purpose: differential coverage for os.path expandvars/normpath."""

import os


def main():
    os.environ["MOLT_PATH_VAR"] = "a/b"
    print("expand", os.path.expandvars("$MOLT_PATH_VAR/c"))
    print("expand_braced", os.path.expandvars("${MOLT_PATH_VAR}/d"))
    print("norm", os.path.normpath("a//b/../c"))


if __name__ == "__main__":
    main()
