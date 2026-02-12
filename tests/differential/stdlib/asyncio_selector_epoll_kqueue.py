"""Purpose: differential coverage for asyncio epoll/kqueue selectors."""

import asyncio
import selectors
import socket


def _probe(name: str, selector_cls):
    if selector_cls is None:
        return (name, "missing")

    selector = None
    loop = None
    reader_sock = None
    writer_sock = None
    try:
        selector = selector_cls()
        loop = asyncio.SelectorEventLoop(selector)
        reader_sock, writer_sock = socket.socketpair()
        reader_sock.setblocking(False)
        writer_sock.setblocking(False)

        fut: asyncio.Future[bytes] = loop.create_future()

        def on_readable() -> None:
            data = reader_sock.recv(1)
            if not fut.done():
                fut.set_result(data)
            loop.remove_reader(reader_sock.fileno())

        loop.add_reader(reader_sock.fileno(), on_readable)
        writer_sock.send(b"x")
        result = loop.run_until_complete(asyncio.wait_for(fut, timeout=1.0))
        return (name, "ok", result)
    except NotImplementedError:
        return (name, "unsupported")
    except Exception as exc:  # pragma: no cover - diagnostic only
        return (name, f"error:{type(exc).__name__}")
    finally:
        if loop is not None:
            try:
                loop.stop()
            except Exception:
                pass
            try:
                loop.close()
            except Exception:
                pass
        if selector is not None:
            try:
                selector.close()
            except Exception:
                pass
        if reader_sock is not None:
            try:
                reader_sock.close()
            except Exception:
                pass
        if writer_sock is not None:
            try:
                writer_sock.close()
            except Exception:
                pass


def main() -> None:
    probes = [
        ("epoll", getattr(selectors, "EpollSelector", None)),
        ("kqueue", getattr(selectors, "KqueueSelector", None)),
    ]
    results = [_probe(name, selector_cls) for name, selector_cls in probes]
    print(results)


if __name__ == "__main__":
    main()
