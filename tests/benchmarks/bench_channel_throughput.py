import asyncio

from molt import intrinsics as _intrinsics

molt_chan_new = _intrinsics.require("molt_chan_new", globals())
molt_chan_recv = _intrinsics.require("molt_chan_recv", globals())
molt_chan_send = _intrinsics.require("molt_chan_send", globals())


async def work() -> int:
    chan = molt_chan_new(0)
    total = 0
    i = 0
    while i < 50_000:
        molt_chan_send(chan, i)
        total += molt_chan_recv(chan)
        i += 1
    return total


def main() -> None:
    print(asyncio.run(work()))


if __name__ == "__main__":
    main()
