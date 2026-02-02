"""Purpose: differential coverage for socket basic."""

import socket


sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.bind(("127.0.0.1", 0))
addr = sock.getsockname()
print(addr[0] == "127.0.0.1", isinstance(addr[1], int))
sock.listen(1)
sock.close()
