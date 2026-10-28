# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: os/pathlib `mode` and wait-status argument conversion parity.

CPython converts each of these arguments with its `int` clinic converter
(`_PyLong_AsInt` after `__index__`): non-integers raise TypeError, an
`__index__` error propagates, and values outside C int raise OverflowError.
os.makedirs gives only the leaf `mode`, so a bad mode raises after missing
ancestors exist; pathlib's mkdir converts `mode` before creating anything.
"""

from __future__ import annotations

import os
import pathlib
import tempfile


class Index:
    def __init__(self, value: int) -> None:
        self.value = value

    def __index__(self) -> int:
        return self.value


class IndexRaises:
    def __index__(self) -> int:
        raise RuntimeError("mode __index__ called")


def show(label: str, action) -> None:
    try:
        result = action()
    except OSError as exc:
        # OSError text carries host paths; the class pins the errno family.
        print(label, type(exc).__name__)
    except Exception as exc:
        print(label, type(exc).__name__, exc)
    else:
        print(label, "ok", result)


root = tempfile.mkdtemp(prefix="molt_mode_conv_")


def at(*parts: str) -> str:
    return os.path.join(root, *parts)


try:
    with open(at("file"), "w") as handle:
        handle.write("x")

    show("mkdir_float", lambda: os.mkdir(at("m_float"), 1.5))
    show("mkdir_index", lambda: os.mkdir(at("m_index"), Index(0o755)))
    show("mkdir_index_isdir", lambda: os.path.isdir(at("m_index")))
    show("mkdir_bool", lambda: os.mkdir(at("m_bool"), True))
    show("mkdir_overflow", lambda: os.mkdir(at("m_big"), 2**40))
    show("mkdir_negative_overflow", lambda: os.mkdir(at("m_neg_big"), -(2**40)))
    show("mkdir_exists", lambda: os.mkdir(at("m_index")))

    show("makedirs_exists", lambda: os.makedirs(at("m_index")))
    show("makedirs_exists_ok", lambda: os.makedirs(at("m_index"), exist_ok=True))
    show("makedirs_file_exist_ok", lambda: os.makedirs(at("file"), exist_ok=True))
    show("makedirs_curdir_tail", lambda: os.makedirs(at("d1", "d2", ".")))
    show("makedirs_curdir_isdir", lambda: os.path.isdir(at("d1", "d2")))
    show("makedirs_trailing_sep", lambda: os.makedirs(at("t1", "t2") + os.sep))
    show("makedirs_trailing_isdir", lambda: os.path.isdir(at("t1", "t2")))
    show("makedirs_bad_mode", lambda: os.makedirs(at("p1", "leaf"), "755"))
    show("makedirs_bad_mode_parent", lambda: os.path.isdir(at("p1")))
    show("makedirs_bad_mode_leaf", lambda: os.path.isdir(at("p1", "leaf")))
    show("makedirs_overflow", lambda: os.makedirs(at("p2", "leaf"), 2**40))
    show("makedirs_index", lambda: os.makedirs(at("p3", "leaf"), Index(0o755)))
    show("makedirs_index_isdir", lambda: os.path.isdir(at("p3", "leaf")))

    show("chmod_str", lambda: os.chmod(at("file"), "644"))
    show("chmod_float", lambda: os.chmod(at("file"), 6.5))
    show("chmod_index_raises", lambda: os.chmod(at("file"), IndexRaises()))
    show("chmod_overflow", lambda: os.chmod(at("file"), 2**40))

    show("path_mkdir_str", lambda: pathlib.Path(at("pm1")).mkdir("755"))
    show("path_mkdir_overflow", lambda: pathlib.Path(at("pm2")).mkdir(2**40))
    show(
        "path_mkdir_parents_bad_mode",
        lambda: pathlib.Path(at("pm3", "leaf")).mkdir(IndexRaises(), parents=True),
    )
    show("path_mkdir_parents_bad_mode_parent", lambda: os.path.isdir(at("pm3")))
    show("path_mkdir_index", lambda: pathlib.Path(at("pm4")).mkdir(Index(0o755)))

    show("wifexited_str", lambda: os.WIFEXITED("0"))
    show("wifexited_index", lambda: os.WIFEXITED(Index(0)))
    show("wifexited_overflow", lambda: os.WIFEXITED(2**40))
    show("wexitstatus_float", lambda: os.WEXITSTATUS(1.5))
    show("wexitstatus", lambda: os.WEXITSTATUS(0x0300))
    show("wifsignaled_killed", lambda: os.WIFSIGNALED(9))
    show("wifsignaled_stopped", lambda: os.WIFSIGNALED(0x057F))
    show("wifsignaled_exited", lambda: os.WIFSIGNALED(0x0100))
    show("wtermsig", lambda: os.WTERMSIG(Index(9)))
    show("wifstopped", lambda: os.WIFSTOPPED(0x057F))
    show("wstopsig", lambda: os.WSTOPSIG(0x057F))
    show("wstopsig_index_raises", lambda: os.WSTOPSIG(IndexRaises()))
finally:
    for dirpath, dirnames, filenames in os.walk(root, topdown=False):
        for name in filenames:
            os.unlink(os.path.join(dirpath, name))
        for name in dirnames:
            os.rmdir(os.path.join(dirpath, name))
    os.rmdir(root)
print("cleanup", os.path.exists(root))
