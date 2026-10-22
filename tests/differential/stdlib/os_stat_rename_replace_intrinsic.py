# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for os.stat/lstat/fstat/rename/replace."""

from __future__ import annotations

import os
import stat
import tempfile
from pathlib import Path


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
        print("stat_fd_size", os.stat(fd).st_size)
        print(
            "fd_predicates",
            os.path.exists(fd),
            os.path.isfile(fd),
            os.path.isdir(fd),
        )
        # Metadata must borrow the descriptor; it remains usable by its owner.
        print("fd_still_open", handle.read())

    class PathLike:
        def __init__(self, path):
            self.path = path
            self.calls = 0

        def __fspath__(self):
            self.calls += 1
            return self.path

    for operation in (os.stat, os.lstat, os.path.exists, os.path.isfile, os.path.isdir):
        path = PathLike(initial)
        result = operation(path)
        print("path_protocol", path.calls, type(result).__name__)

    for predicate in (os.path.exists, os.path.isfile, os.path.isdir, os.path.islink):
        print("nul_predicate", predicate(initial + "\0"))
        try:
            predicate(object())
        except TypeError:
            print("invalid_path_type", "TypeError")

    link = os.path.join(root, "link.txt")
    if os.name != "nt":
        # Native Windows link creation requires an independently granted host
        # privilege; the ordinary Unix differential cell exercises real links.
        os.symlink("alpha.txt", link)
        try:
            print(
                "link_kinds",
                stat.S_ISREG(os.stat(link).st_mode),
                stat.S_ISLNK(os.lstat(link).st_mode),
            )
            print(
                "link_predicates",
                os.path.exists(link),
                os.path.isfile(link),
                os.path.isdir(link),
                os.path.islink(link),
            )
            for path in (link, os.fsencode(link)):
                target = os.readlink(path)
                print("link_flavor", type(target).__name__, target)
        finally:
            os.unlink(link)

    # The visible tuple stores integral seconds; the attribute and ns values
    # are independent native fields. The chosen ns values reject float-derived
    # reconstruction while remaining representable by common filesystems.
    stamp = 1_700_000_000_123_456_789
    os.utime(initial, ns=(stamp, stamp + 123_456_789))
    precise = os.stat(initial)
    with open(initial, "rb") as handle:
        descriptor = os.fstat(handle.fileno())
    print(
        "stat_time_coordinates",
        isinstance(precise[7], int),
        isinstance(precise.st_atime, float),
        precise[7] == precise.st_atime_ns // 1_000_000_000,
        precise[8] == precise.st_mtime_ns // 1_000_000_000,
    )
    print(
        "stat_ns_native_precision",
        precise.st_atime_ns == stamp,
        precise.st_mtime_ns == stamp + 123_456_789,
    )
    print(
        "stat_same_snapshot",
        precise.st_atime_ns == descriptor.st_atime_ns,
        precise.st_mtime_ns == descriptor.st_mtime_ns,
        precise.st_ino == descriptor.st_ino,
        precise.st_dev == descriptor.st_dev,
        precise.st_nlink == descriptor.st_nlink,
    )
    print(
        "stat_field_geometry",
        precise.n_sequence_fields,
        precise.n_unnamed_fields,
        len(precise),
        precise.n_fields > len(precise),
    )
    fabricated = os.stat_result(
        tuple(range(10)), {"st_atime": 70.25, "st_mtime_ns": 8_987_654_321}
    )
    print(
        "stat_constructor_hidden",
        fabricated[7],
        fabricated.st_atime,
        fabricated.st_mtime,
        fabricated.st_mtime_ns,
        fabricated.st_atime_ns,
    )
    cls = os.stat_result
    os.stat_result = object
    try:
        print(
            "stat_captured_type",
            type(os.stat(initial)) is cls,
            type(Path(initial).stat()) is cls,
            type(Path(initial).lstat()) is cls,
        )
    finally:
        os.stat_result = cls

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
