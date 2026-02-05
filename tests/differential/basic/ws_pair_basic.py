# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import asyncio
import socket


async def main() -> None:
    loop = asyncio.get_running_loop()
    left_sock, right_sock = socket.socketpair()
    left_sock.setblocking(False)
    right_sock.setblocking(False)
    try:
        await loop.sock_sendall(left_sock, b"ping")
        msg = await loop.sock_recv(right_sock, 4)
        assert msg == b"ping"
        await loop.sock_sendall(right_sock, b"pong")
        msg = await loop.sock_recv(left_sock, 4)
        assert msg == b"pong"
    finally:
        left_sock.close()
        right_sock.close()


asyncio.run(main())
