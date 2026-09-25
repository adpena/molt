# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""File-stream public I/O, bounded pulls, closure, and async context semantics."""

import asyncio
import os

from moltlib.io import stream


path = f"_molt_file_stream_{os.getpid()}.txt"
with open(path, "xb") as handle:
    handle.write(b"abcdefg")

try:
    with stream(path, chunk_size=3) as reader:
        print(list(reader))
        print("eof-closed", reader.closed)
    with stream(path, "r", chunk_size=2, encoding="ascii") as reader:
        print(list(reader))
    for size in (0, -1):
        try:
            stream(path, "wb", chunk_size=size)
        except ValueError:
            print("invalid-chunk", size)
    with open(path, "rb") as handle:
        print("preserved", handle.read())

    async def consume():
        async with stream(path, chunk_size=2) as reader:
            async for chunk in reader:
                print("first", chunk)
                break
        print("early-closed", reader.closed)
        try:
            await anext(reader)
        except StopAsyncIteration:
            print("async-exhausted")
        await reader.aclose()

    asyncio.run(consume())
finally:
    os.unlink(path)
