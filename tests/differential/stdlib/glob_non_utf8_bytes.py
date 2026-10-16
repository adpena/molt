# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Differential coverage for non-UTF8 glob bytes behavior across hosts."""

from __future__ import annotations

import glob
import os
import sys
import tempfile


with tempfile.TemporaryDirectory(prefix="molt_glob_nonutf8_") as root:
    broot = os.fsencode(root)
    bad_name = b"\xff.txt"
    bad_path = os.path.join(broot, bad_name)

    print("platform", os.name)
    print("fs_encode_errors", sys.getfilesystemencodeerrors())
    print("root_bytes_type", type(broot).__name__)

    created = False
    try:
        fd = os.open(bad_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
    except Exception as exc:
        print("create_bad_exc", type(exc).__name__)
    else:
        try:
            os.write(fd, b"x")
        finally:
            os.close(fd)
        created = True
        print("create_bad_ok", True)

    try:
        matches = sorted(glob.glob(b"*.txt", root_dir=broot))
    except Exception as exc:
        print("glob_bytes_exc", type(exc).__name__)
    else:
        print("glob_bytes_ok", True)
        print("contains_bad", bad_name in matches)
        print("matches_len", len(matches))
        if created:
            print("contains_bad_after_create", bad_name in matches)

    print("escape_bad", glob.escape(bad_name))

# Text patterns count Python characters; byte patterns count encoded bytes.
# Creation uses the existing stream owner and does not depend on fd export.
if os.name == "posix":
    from pathlib import Path

    with tempfile.TemporaryDirectory(prefix="molt_glob_units_") as root:
        broot = os.fsencode(root)
        created_names = []
        for name in (b"\xff.one", "\xe9.one".encode("utf-8"), b"ab.one"):
            try:
                with open(os.path.join(broot, name), "wb") as stream:
                    stream.write(b"payload")
            except OSError as error:
                print("name-refusal", name, type(error).__name__)
            else:
                created_names.append(name)
        print("text-units", sorted(glob.glob("?.one", root_dir=root)))
        print("byte-units", sorted(glob.glob(b"?.one", root_dir=broot)))
        print("two-byte-units", sorted(glob.glob(b"??.one", root_dir=broot)))
        print(
            "listdir-roundtrip", sorted(os.fsencode(name) for name in os.listdir(root))
        )
        print(
            "pathlib-roundtrip",
            sorted(os.fsencode(os.fspath(path)) for path in Path(root).iterdir())
            == sorted(os.path.join(broot, name) for name in os.listdir(broot)),
        )
        print(
            "pathlib-glob-units",
            sorted(os.fsencode(os.fspath(path)) for path in Path(root).glob("?.one"))
            == sorted(
                os.path.join(broot, name) for name in created_names if name != b"ab.one"
            ),
        )

        # The public facades preserve the runtime-selected string/bytes flavor.
        for directory in (root, broot):
            names = sorted(os.listdir(directory))
            entries = list(os.scandir(directory))
            print("scandir-names", sorted(entry.name for entry in entries) == names)
            print(
                "scandir-paths",
                all(entry.path == os.path.join(directory, entry.name)
                    and os.fspath(entry) == entry.path for entry in entries),
            )
            walked = list(os.walk(directory))
            print("walk-flavor", len(walked) == 1 and walked[0][0] == directory
                  and walked[0][1] == [] and sorted(walked[0][2]) == names)
        print("commonpath-bytes", os.path.commonpath(
            [broot, os.path.join(broot, b"\xff.one")]) == broot)

        class BytePath:
            calls = 0

            def __fspath__(self):
                self.calls += 1
                return broot

            def __str__(self):
                raise AssertionError("path protocol replaced by str")

        pathlike = BytePath()
        print("pathlike-listdir", sorted(os.listdir(pathlike)) == sorted(created_names))
        print("pathlike-scandir", sorted(entry.name for entry in os.scandir(pathlike))
              == sorted(created_names))
        print("pathlike-walk", sorted(next(os.walk(pathlike))[2]) == sorted(created_names))
        print("pathlike-calls", pathlike.calls)
