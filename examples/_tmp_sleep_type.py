import asyncio

obj = asyncio.sleep(0)
print("sleep_type", type(obj).__name__)
print("sleep_obj", obj)
