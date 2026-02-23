# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for os.mkdir/os.makedirs mode handling."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_mkdir_mode_")
mkdir_leaf = os.path.join(root, "mkdir_leaf")
makedirs_parent = os.path.join(root, "makedirs_parent")
makedirs_leaf = os.path.join(makedirs_parent, "makedirs_leaf")


class IndexRaisesMode:
    def __index__(self) -> int:
        raise RuntimeError("mode __index__ called")


def expect_type_error(label: str, action) -> None:
    try:
        action()
    except Exception as exc:
        print(label, type(exc).__name__)
    else:
        print(label, "no-error")


try:
    expect_type_error("mkdir_bad_mode", lambda: os.mkdir(os.path.join(root, "bad_mkdir"), "755"))
    expect_type_error(
        "makedirs_bad_mode",
        lambda: os.makedirs(os.path.join(root, "bad_makedirs"), "755"),
    )
    expect_type_error(
        "mkdir_mode_index_raises",
        lambda: os.mkdir(os.path.join(root, "mkdir_index_raises"), IndexRaisesMode()),
    )
    expect_type_error(
        "makedirs_mode_index_raises",
        lambda: os.makedirs(
            os.path.join(root, "makedirs_index_raises", "leaf"),
            IndexRaisesMode(),
        ),
    )

    os.mkdir(mkdir_leaf, 0)
    print("mkdir_exists", os.path.isdir(mkdir_leaf))

    os.makedirs(makedirs_leaf, 0)
    print("makedirs_leaf_exists", os.path.isdir(makedirs_leaf))

    print("mode_bits_check", "deferred_no_os_stat")
finally:
    for path in (makedirs_leaf, makedirs_parent, mkdir_leaf, root):
        try:
            os.rmdir(path)
        except Exception:
            pass
