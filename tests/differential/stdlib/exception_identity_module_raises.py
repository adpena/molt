"""Purpose: runtime raises of module-owned classes produce the module's own class."""

import asyncio
import dataclasses
import http.client
import xml.etree.ElementTree as ET


@dataclasses.dataclass(frozen=True)
class Point:
    x: int


def probe(label, func, expected):
    try:
        func()
    except expected as exc:
        print(label, type(exc) is expected, type(exc).__module__, type(exc).__mro__[1].__name__)
    except Exception as exc:  # noqa: BLE001 - the observation is the escape
        print(label, "escaped", type(exc).__name__, type(exc).__module__)
    else:
        print(label, "no error")


def assign():
    point = Point(1)
    point.x = 2


def invalid_url():
    http.client.HTTPConnection("localhost", 1).putrequest("GET", "/a b")


def queue_full():
    queue = asyncio.Queue(maxsize=1)
    queue.put_nowait(1)
    queue.put_nowait(2)


def queue_empty():
    asyncio.Queue().get_nowait()


def main():
    probe("frozen", assign, dataclasses.FrozenInstanceError)
    probe("frozen again", assign, dataclasses.FrozenInstanceError)
    probe("invalid url", invalid_url, http.client.InvalidURL)
    probe("parse error", lambda: ET.fromstring(""), ET.ParseError)
    probe("queue full", queue_full, asyncio.QueueFull)
    probe("queue empty", queue_empty, asyncio.QueueEmpty)


if __name__ == "__main__":
    main()
