"""Purpose: differential coverage for socket.dup timeout/blocking clone semantics."""

import socket


left, right = socket.socketpair()
try:
    left.settimeout(1.25)
    clone = left.dup()
    try:
        print("dup_timeout", clone.gettimeout())
    finally:
        clone.close()

    left.setblocking(False)
    clone2 = left.dup()
    try:
        print("dup_blocking", clone2.getblocking())
        print("dup_timeout_nonblocking", clone2.gettimeout())
    finally:
        clone2.close()
finally:
    left.close()
    right.close()
