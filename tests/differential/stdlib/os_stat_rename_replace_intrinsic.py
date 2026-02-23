# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for os.stat/lstat/fstat/rename/replace."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_stat_ops_")
initial = os.path.join(root, "alpha.txt")
renamed = os.path.join(root, "beta.txt")
replace_src = os.path.join(root, "replace_src.txt")
replace_dst = os.path.join(root, "replace_dst.txt")

fd = -1

try:
    with open(initial, "wb") as handle:
        handle.write(b"alpha")

    stat_result = os.stat(initial)
    lstat_result = os.lstat(initial)
    print("stat_type", type(stat_result).__name__, type(lstat_result).__name__)
    print("stat_shape", len(stat_result), len(lstat_result))
    print("stat_sizes", stat_result[6], lstat_result[6])
    print(
        "stat_attrs",
        isinstance(stat_result.st_mode, int),
        isinstance(stat_result.st_atime, float),
        isinstance(stat_result.st_atime_ns, int),
    )

    with open(initial, "rb") as handle:
        fd = handle.fileno()
        fstat_result = os.fstat(fd)
        print("fstat_shape", len(fstat_result), type(fstat_result).__name__)
        print("fstat_size", fstat_result[6])

    follow_false = os.stat(initial, follow_symlinks=False)
    print("follow_symlinks_false_size", follow_false[6])

    os.rename(initial, renamed)
    print("rename_paths", os.path.exists(initial), os.path.exists(renamed))

    with open(replace_src, "wb") as handle:
        handle.write(b"new-data")
    with open(replace_dst, "wb") as handle:
        handle.write(b"old-data")

    os.replace(replace_src, replace_dst)
    with open(replace_dst, "rb") as handle:
        print("replace_payload", handle.read())

    try:
        os.fstat(-1)
    except OSError as exc:
        print("fstat_badfd", type(exc).__name__)
finally:
    fd = -1
    for path in (replace_src, replace_dst, renamed, initial):
        try:
            os.unlink(path)
        except Exception:
            pass
    try:
        os.rmdir(root)
    except Exception:
        pass
