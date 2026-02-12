# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
"""Purpose: differential coverage for TCP half-close behavior."""

import socket
import threading


ready = threading.Event()
port_holder: list[int] = []
received: list[bytes] = []


def server() -> None:
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", 0))
    port_holder.append(srv.getsockname()[1])
    srv.listen(1)
    ready.set()

    conn, _addr = srv.accept()
    chunks: list[bytes] = []
    while True:
        data = conn.recv(1024)
        if not data:
            break
        chunks.append(data)
    received.append(b"".join(chunks))
    conn.sendall(b"ack")
    conn.close()
    srv.close()


t = threading.Thread(target=server)
t.start()
ready.wait(timeout=1.0)

sock = socket.create_connection(("127.0.0.1", port_holder[0]))
sock.sendall(b"payload")
sock.shutdown(socket.SHUT_WR)
ack = sock.recv(1024)
sock.close()
t.join()

print(received[0].decode("ascii"))
print(ack.decode("ascii"))
