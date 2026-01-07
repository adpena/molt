import asyncio

from molt.shims import molt_chan_new, molt_chan_recv, molt_chan_send


async def work() -> int:
    chan = molt_chan_new(0)
    total = 0
    i = 0
    while i < 100_000:
        molt_chan_send(chan, i)
        total += molt_chan_recv(chan)
        i += 1
    return total


print(asyncio.run(work()))
