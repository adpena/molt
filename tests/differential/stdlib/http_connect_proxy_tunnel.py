# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for http connect proxy tunnel."""

import http.client
import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
received: list[bytes] = []


def proxy() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    data = conn.recv(4096)
    received.append(data)
    # Minimal proxy response to CONNECT
    conn.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
    # Echo a payload through the tunnel
    tunneled = conn.recv(1024)
    received.append(tunneled)
    conn.sendall(b"pong")
    conn.close()
    srv.close()


t = threading.Thread(target=proxy)
t.start()
ready.wait(timeout=1.0)

conn = http.client.HTTPConnection("127.0.0.1", port_holder[0], timeout=1.0)
conn.set_tunnel("example.com", 443)
conn.connect()
# After connect(), the tunnel should be up; send raw bytes
conn.sock.sendall(b"ping")
resp = conn.sock.recv(1024)
conn.close()

t.join()

print(received[0].split(b"\r\n")[0].decode())
print(resp.decode())
