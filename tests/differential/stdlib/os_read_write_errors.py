"""Purpose: differential coverage for os.read/os.write error mapping."""

import os

try:
    os.read(-1, 1)
except OSError as exc:
    print("read_badfd", type(exc).__name__)

try:
    os.read(0, -1)
except Exception as exc:
    print("read_negative", type(exc).__name__, getattr(exc, "errno", None))

rfd, wfd = os.pipe()
try:
    try:
        os.write(wfd, "text")
    except Exception as exc:
        print("write_type", type(exc).__name__)
finally:
    os.close(rfd)
    os.close(wfd)
