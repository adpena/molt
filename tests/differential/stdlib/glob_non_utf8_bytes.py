# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
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
