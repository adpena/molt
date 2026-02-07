"""Purpose: differential coverage for os.read/os.write fd semantics."""

import os

rfd, wfd = os.pipe()
try:
    view = memoryview(b"abcdef")
    wrote = os.write(wfd, view)
    print("wrote", wrote)
    print("read1", os.read(rfd, 2))
    print("read2", os.read(rfd, 16))
    print("read0", os.read(rfd, 0))
finally:
    os.close(rfd)
    os.close(wfd)
