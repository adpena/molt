import errno
import io
import os


path = "_molt_io_isatty.txt"
with open(path, "w", encoding="utf-8") as handle:
    handle.write("hello")

try:
    with open(path, "r", encoding="utf-8") as handle:
        print("isatty-file", handle.isatty())
    missing = path + ".missing"
    assert not os.path.exists(missing)
    try:
        open(missing, "rb")
    except FileNotFoundError as exc:
        print("missing-file-domain", type(exc) is FileNotFoundError, exc.errno == errno.ENOENT)
    else:
        raise AssertionError("missing file unexpectedly opened")

    # The differential harness captures stdout through a pipe. Querying its
    # current offset changes no position if a broken fixture supplies a file.
    output = io.FileIO(1, "w", closefd=False)
    try:
        output.seek(0, os.SEEK_CUR)
    except OSError as exc:
        expected = errno.EINVAL if os.name == "nt" else errno.ESPIPE
        print("pipe-seek-domain", type(exc) is OSError, exc.errno == expected)
    else:
        raise AssertionError("captured stdout pipe unexpectedly seekable")
    finally:
        output.close()

finally:
    try:
        os.unlink(path)
    except FileNotFoundError:
        pass
