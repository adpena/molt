# MOLT_ENV: MOLT_CAPABILITIES=net.listen,net.outbound
import asyncio
import socket


async def main() -> None:
    loop = asyncio.get_running_loop()
    left_sock, right_sock = socket.socketpair()
    left_sock.setblocking(False)
    right_sock.setblocking(False)
    try:
        await loop.sock_sendall(left_sock, b"hello")
        buf = bytearray(5)
        n = await loop.sock_recv_into(right_sock, buf)
        assert n == 5
        assert bytes(buf) == b"hello"

        await loop.sock_sendall(right_sock, b"world")
        buf2 = bytearray(5)
        n2 = await loop.sock_recv_into(left_sock, buf2)
        assert n2 == 5
        assert bytes(buf2) == b"world"
    finally:
        left_sock.close()
        right_sock.close()


asyncio.run(main())
