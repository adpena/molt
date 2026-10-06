"""Draft extension of asyncio_run_shutdown_asyncgens.py for loop ownership."""
import asyncio
log = []
held = []

async def values(label):
    try:
        yield label + '-first'
        await asyncio.sleep(0)
        yield label + '-second'
    finally:
        await asyncio.sleep(0)
        log.append(label)

async def open_generator(label):
    gen = values(label)
    print('open', await gen.__anext__())
    held.append(gen)
    return gen

async def advance(gen):
    try:
        print('next', await gen.__anext__())
    except StopAsyncIteration:
        print('next', 'closed')

first = asyncio.Runner()
second = asyncio.Runner()
a = first.run(open_generator('a'))
b = second.run(open_generator('b'))
print('before-close', log)
first.close()
print('first-close', log, a.ag_frame is None, b.ag_frame is None)
second.run(advance(a))
second.run(advance(b))
second.close()
print('second-close', log, a.ag_frame is None, b.ag_frame is None, len(held))
