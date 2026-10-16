# MOLT_ENV: MOLT_CAPABILITIES=env.read,env.write
"""Purpose: differential coverage for os.path expandvars/normpath."""

import os


def main():
    os.environ["MOLT_PATH_VAR"] = "a/b"
    print("expand", os.path.expandvars("$MOLT_PATH_VAR/c"))
    print("expand_braced", os.path.expandvars("${MOLT_PATH_VAR}/d"))
    print("norm", os.path.normpath("a//b/../c"))
    for value in (
        "$MOLT_PATH_VAR/${MOLT_PATH_UNKNOWN}",
        "é😀/$MOLT_PATH_VAR/尾/${MOLT_PATH_VAR}",
        "$$MOLT_PATH_VAR/${MOLT_PATH_VAR}/$é",
        "${unfinished/$MOLT_PATH_VAR",
        "'%MOLT_PATH_VAR%'/%MOLT_PATH_VAR%/%%/$$",
    ):
        print("text", os.path.expandvars(value))
        print("bytes", os.path.expandvars(os.fsencode(value)))
    print("surrogate", repr(os.path.expandvars("\ud800/$MOLT_PATH_VAR/\udcff")))
    if os.supports_bytes_environ:
        os.environb[b"MOLT_PATH_RAW"] = b"\xff\x80"
        print("raw", os.path.expandvars(b"\xfe/${MOLT_PATH_RAW}/$MOLT_PATH_VAR"))
    long_path = ("é😀/$MOLT_PATH_VAR/" * 256) + "尾"
    print("long", os.path.expandvars(long_path) == ("é😀/a/b/" * 256) + "尾")


if __name__ == "__main__":
    main()
