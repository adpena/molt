# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for socket udp echo."""

import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
received: list[str] = []
addr_ok: list[bool] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    ready.set()
    data, addr = srv.recvfrom(1024)
    received.append(data.decode())
    addr_ok.append(addr[0] == "127.0.0.1")
    srv.sendto(b"pong", addr)
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.settimeout(1.0)
sock.sendto(b"ping", ("127.0.0.1", port_holder[0]))
resp, _addr = sock.recvfrom(1024)
sock.close()

t.join()

print("recv", received[0])
print("addr_ok", addr_ok[0])
print("resp", resp.decode())
