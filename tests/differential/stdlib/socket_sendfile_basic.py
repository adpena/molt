"""Purpose: differential coverage for socket.sendfile intrinsic path."""

import os
import socket
import tempfile

# Create a temp file with known content
content = b"hello sendfile world\n" * 100
with tempfile.NamedTemporaryFile(delete=False) as tmp:
    tmp.write(content)
    tmp_path = tmp.name

try:
    left, right = socket.socketpair()
    try:
        with open(tmp_path, "rb") as f:
            sent = left.sendfile(f)
            print(isinstance(sent, int))
            print(sent == len(content))
        left.shutdown(socket.SHUT_WR)
        received = b""
        while True:
            chunk = right.recv(4096)
            if not chunk:
                break
            received += chunk
        print(len(received) == len(content))
        print(received == content)
    finally:
        left.close()
        right.close()
finally:
    os.unlink(tmp_path)

# Test sendfile with offset and count
content2 = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ"
with tempfile.NamedTemporaryFile(delete=False) as tmp2:
    tmp2.write(content2)
    tmp_path2 = tmp2.name

try:
    left2, right2 = socket.socketpair()
    try:
        with open(tmp_path2, "rb") as f:
            sent2 = left2.sendfile(f, offset=5, count=10)
            print(sent2 == 10)
        left2.shutdown(socket.SHUT_WR)
        received2 = b""
        while True:
            chunk = right2.recv(4096)
            if not chunk:
                break
            received2 += chunk
        print(received2 == b"FGHIJKLMNO")
    finally:
        left2.close()
        right2.close()
finally:
    os.unlink(tmp_path2)

# Test error: no fileno
try:
    s = socket.socket()
    s.sendfile("not a file")
    print("no error")
except (OSError, TypeError) as e:
    print("error raised")
finally:
    s.close()
