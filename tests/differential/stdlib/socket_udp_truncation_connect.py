# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for UDP connect/truncation."""

import socket

srv = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
srv.bind(("127.0.0.1", 0))
addr = srv.getsockname()

cli = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
cli.sendto(b"hello", addr)

data, peer = srv.recvfrom(1024)
print(data, peer[0] == "127.0.0.1")

cli.connect(addr)
print(cli.getpeername()[1] == addr[1])
cli.send(b"abcd")
print(srv.recvfrom(2)[0])

try:
    cli.connect(("0.0.0.0", 0))
    print("disconnected")
except OSError as exc:
    print(type(exc).__name__)

cli.close()
srv.close()
