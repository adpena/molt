# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Purpose: differential coverage for os.utime(ns=...) without dir_fd."""

from __future__ import annotations

import os
import tempfile


root = tempfile.mkdtemp(prefix="molt_os_utime_ns_")
target = os.path.join(root, "payload.txt")

try:
    with open(target, "wb") as handle:
        handle.write(b"ns-only")

    os.utime(target, ns=(2_000_000_000_000, 3_000_000_000_000))
    st = os.stat(target)
    print("atime_sec", int(st.st_atime))
    print("mtime_sec", int(st.st_mtime))

    # times= and ns= are mutually exclusive.
    try:
        os.utime(target, times=(1.0, 2.0), ns=(1, 2))
    except ValueError as exc:
        print("mutually_exclusive", type(exc).__name__)

    # A timestamp beyond platform time_t overflows.
    try:
        os.utime(target, ns=(10 ** 30, 10 ** 30))
    except OverflowError as exc:
        print("overflow", type(exc).__name__, str(exc))
finally:
    try:
        os.unlink(target)
    except Exception:
        pass
    try:
        os.rmdir(root)
    except Exception:
        pass
