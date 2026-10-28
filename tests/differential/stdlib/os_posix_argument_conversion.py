# MOLT_META: backends=jit,llvm,native platforms=linux,macos
# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: os fd, size, offset and permission argument conversion parity.

CPython's clinic converters: `fd`/`flags`/`mode`/`mask` use `int`
(`_PyLong_AsInt`), os.read's length uses `Py_ssize_t`, and lseek/truncate/
ftruncate offsets use `Py_off_t`. Each admits `__index__`, rejects other
types with TypeError and out-of-range values with OverflowError. A negative
length reaches the system call, which fails with EINVAL (OSError). Every
operation here needs a POSIX host: fd I/O, truncation and umask are native-only
in Molt's capability table, and WASI has no chmod.
"""

from __future__ import annotations

import os
import tempfile


class Index:
    def __init__(self, value: int) -> None:
        self.value = value

    def __index__(self) -> int:
        return self.value


def show(label: str, action) -> None:
    try:
        result = action()
    except OSError as exc:
        print(label, type(exc).__name__, exc.errno)
    except Exception as exc:
        print(label, type(exc).__name__, exc)
    else:
        print(label, "ok", result)


handle, path = tempfile.mkstemp(prefix="molt_fd_conv_")
os.close(handle)
try:
    with open(path, "w") as stream:
        stream.write("abc")

    show("open_flags_str", lambda: os.open(path, "0"))
    show("open_flags_overflow", lambda: os.open(path, 2**40))
    show("open_mode_str", lambda: os.open(path, os.O_RDONLY, "0"))
    show("open_mode_overflow", lambda: os.open(path, os.O_RDONLY, 2**40))
    fd = os.open(path, Index(os.O_RDWR), Index(0o600))
    print("open_index", fd >= 0)

    show("read_length_str", lambda: os.read(fd, "1"))
    show("read_length_float", lambda: os.read(fd, 1.0))
    show("read_length_overflow", lambda: os.read(fd, 2**70))
    show("read_length_negative", lambda: os.read(fd, -1))
    show("read_fd_overflow", lambda: os.read(2**40, 1))
    show("read_index", lambda: os.read(Index(fd), Index(2)))

    show("lseek_pos_str", lambda: os.lseek(fd, "0", os.SEEK_SET))
    show("lseek_pos_overflow", lambda: os.lseek(fd, 2**70, os.SEEK_SET))
    show("lseek_index", lambda: os.lseek(fd, Index(1), os.SEEK_SET))

    show("ftruncate_fd_str", lambda: os.ftruncate("0", 0))
    show("ftruncate_fd_float", lambda: os.ftruncate(float(fd), 0))
    show("ftruncate_length_str", lambda: os.ftruncate(fd, "0"))
    show("ftruncate_length_overflow", lambda: os.ftruncate(fd, 2**70))
    show("ftruncate_length_negative", lambda: os.ftruncate(fd, -1))
    show("ftruncate_index", lambda: os.ftruncate(Index(fd), Index(2)))
    show("ftruncate_size", lambda: os.path.getsize(path))
    os.close(fd)

    show("truncate_length_str", lambda: os.truncate(path, "0"))
    show("truncate_length_float", lambda: os.truncate(path, 1.0))
    show("truncate_length_overflow", lambda: os.truncate(path, 2**70))
    show("truncate_length_negative", lambda: os.truncate(path, -1))
    show("truncate_index", lambda: os.truncate(path, Index(1)))
    show("truncate_size", lambda: os.path.getsize(path))

    show("chmod_index", lambda: os.chmod(path, Index(0o600)))

    show("umask_str", lambda: os.umask("0"))
    show("umask_overflow", lambda: os.umask(2**40))
    previous = os.umask(0o022)
    show("umask_index", lambda: os.umask(Index(previous)) == 0o022)

    show("get_terminal_size_str", lambda: os.get_terminal_size("1"))
    show("get_terminal_size_overflow", lambda: os.get_terminal_size(2**40))
finally:
    os.unlink(path)
print("cleanup", os.path.exists(path))
