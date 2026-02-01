import asyncio

p = asyncio.molt_promise_new()
print("promise", p)
print("has___await__", hasattr(p, "__await__"))
print("type", type(p))
