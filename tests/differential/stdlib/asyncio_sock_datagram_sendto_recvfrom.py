# MOLT_ENV: MOLT_CAPABILITIES=net
import asyncio
import socket


async def main() -> None:
    loop = asyncio.get_running_loop()
    left = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    right = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    left.bind(("127.0.0.1", 0))
    right.bind(("127.0.0.1", 0))
    left.setblocking(False)
    right.setblocking(False)
    try:
        left_addr = left.getsockname()
        right_addr = right.getsockname()

        sent = await loop.sock_sendto(left, b"ping", right_addr)
        assert sent == 4
        data, addr = await loop.sock_recvfrom(right, 4)
        assert data == b"ping"
        assert addr[0] == left_addr[0]

        sent2 = await loop.sock_sendto(right, b"pong", left_addr)
        assert sent2 == 4
        buf = bytearray(4)
        n, addr2 = await loop.sock_recvfrom_into(left, buf)
        assert n == 4
        assert bytes(buf) == b"pong"
        assert addr2[0] == right_addr[0]
    finally:
        left.close()
        right.close()


asyncio.run(main())
