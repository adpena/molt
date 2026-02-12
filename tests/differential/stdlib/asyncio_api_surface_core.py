"""Purpose: differential coverage for asyncio core class/exception surface."""

from __future__ import annotations

import asyncio


def _probe(name: str, getter) -> tuple[str, bool, str | None]:
    try:
        value = getter()
    except AttributeError:
        return (name, False, None)
    except Exception as exc:  # pragma: no cover - diagnostic only
        return (name, True, f"error:{type(exc).__name__}")
    return (name, True, type(value).__name__)


PROBES = [
    ("AbstractChildWatcher", lambda: asyncio.AbstractChildWatcher),
    ("AbstractEventLoopPolicy", lambda: asyncio.AbstractEventLoopPolicy),
    ("BaseEventLoop", lambda: asyncio.BaseEventLoop),
    ("BaseProtocol", lambda: asyncio.BaseProtocol),
    ("BrokenBarrierError", lambda: asyncio.BrokenBarrierError),
    ("BufferedProtocol", lambda: asyncio.BufferedProtocol),
    ("DatagramProtocol", lambda: asyncio.DatagramProtocol),
    ("DatagramTransport", lambda: asyncio.DatagramTransport),
    ("DefaultEventLoopPolicy", lambda: asyncio.DefaultEventLoopPolicy),
    ("EventLoop", lambda: asyncio.EventLoop),
    ("FastChildWatcher", lambda: asyncio.FastChildWatcher),
    ("Handle", lambda: asyncio.Handle),
    ("InvalidStateError", lambda: asyncio.InvalidStateError),
    ("LimitOverrunError", lambda: asyncio.LimitOverrunError),
    ("PidfdChildWatcher", lambda: asyncio.PidfdChildWatcher),
    ("ProactorEventLoop", lambda: asyncio.ProactorEventLoop),
    ("Protocol", lambda: asyncio.Protocol),
    ("QueueEmpty", lambda: asyncio.QueueEmpty),
    ("QueueFull", lambda: asyncio.QueueFull),
    ("QueueShutDown", lambda: asyncio.QueueShutDown),
    ("SafeChildWatcher", lambda: asyncio.SafeChildWatcher),
    ("SelectorEventLoop", lambda: asyncio.SelectorEventLoop),
    ("SendfileNotAvailableError", lambda: asyncio.SendfileNotAvailableError),
    ("StreamReaderProtocol", lambda: asyncio.StreamReaderProtocol),
    ("SubprocessProtocol", lambda: asyncio.SubprocessProtocol),
    ("SubprocessTransport", lambda: asyncio.SubprocessTransport),
    ("ThreadedChildWatcher", lambda: asyncio.ThreadedChildWatcher),
    ("TimerHandle", lambda: asyncio.TimerHandle),
    ("Transport", lambda: asyncio.Transport),
    ("WindowsProactorEventLoopPolicy", lambda: asyncio.WindowsProactorEventLoopPolicy),
    ("WindowsSelectorEventLoopPolicy", lambda: asyncio.WindowsSelectorEventLoopPolicy),
]


def main() -> None:
    results = [_probe(name, getter) for name, getter in PROBES]
    print(results)


if __name__ == "__main__":
    main()
