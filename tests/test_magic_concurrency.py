import asyncio

async def worker(c):
    molt_chan_send(c, 42)

async def main():
    c = molt_chan_new()
    molt_spawn(worker(c))
    print(molt_chan_recv(c))

asyncio.run(main())