"""Purpose: differential coverage for asyncio add_reader/add_writer."""

import asyncio
import socket


async def main() -> None:
    loop = asyncio.get_running_loop()
    reader_sock: socket.socket | None = None
    writer_sock: socket.socket | None = None
    try:
        reader_sock, writer_sock = socket.socketpair()
        reader_sock.setblocking(False)
        writer_sock.setblocking(False)

        reader_future: asyncio.Future[bytes] = loop.create_future()
        writer_future: asyncio.Future[str] = loop.create_future()

        def on_readable() -> None:
            data = reader_sock.recv(1)
            if not reader_future.done():
                reader_future.set_result(data)
            loop.remove_reader(reader_sock.fileno())

        def on_writable() -> None:
            if not writer_future.done():
                writer_future.set_result("writable")
            loop.remove_writer(writer_sock.fileno())

        loop.add_reader(reader_sock.fileno(), on_readable)
        loop.add_writer(writer_sock.fileno(), on_writable)
        writer_sock.send(b"x")

        read_data = await asyncio.wait_for(reader_future, timeout=1.0)
        write_state = await asyncio.wait_for(writer_future, timeout=1.0)
        print(read_data, write_state)
    except NotImplementedError:
        print("unsupported")
    finally:
        if reader_sock is not None:
            try:
                loop.remove_reader(reader_sock.fileno())
            except Exception:
                pass
        if writer_sock is not None:
            try:
                loop.remove_writer(writer_sock.fileno())
            except Exception:
                pass
        try:
            if reader_sock is not None:
                reader_sock.close()
            if writer_sock is not None:
                writer_sock.close()
        except Exception:
            pass


asyncio.run(main())
