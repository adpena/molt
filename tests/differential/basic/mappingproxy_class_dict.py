"""Purpose: differential coverage for class __dict__ mappingproxy behavior."""


class Demo:
    value = 10


print(type(Demo.__dict__).__name__)
print(Demo.__dict__.get("value"))

try:
    Demo.__dict__["other"] = 3
except Exception as exc:
    print(type(exc).__name__, exc)

instance = Demo()
print(type(instance.__dict__).__name__)
instance.__dict__["x"] = 1
print(instance.__dict__["x"])
