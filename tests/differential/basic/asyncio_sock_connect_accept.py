# MOLT_ENV: MOLT_CAPABILITIES=net.bind,net.listen,net.outbound,net.connect
import asyncio
import socket


async def main() -> None:
    loop = asyncio.get_running_loop()

    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", 0))
    server.listen(1)
    server.setblocking(False)

    addr = server.getsockname()
    accept_task = asyncio.create_task(loop.sock_accept(server))

    client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    client.setblocking(False)
    accepted = None
    try:
        await loop.sock_connect(client, addr)
        accepted, _peer = await accept_task
        accepted.setblocking(False)

        await loop.sock_sendall(client, b"ok")
        data = await loop.sock_recv(accepted, 2)
        assert data == b"ok"
    finally:
        if accepted is not None:
            accepted.close()
        client.close()
        server.close()


asyncio.run(main())
