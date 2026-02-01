import asyncio

f = asyncio.molt_async_sleep(0.0)
print("f", f)
print("has___await__", hasattr(f, "__await__"))
print("type", type(f))
