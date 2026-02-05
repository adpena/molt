import types


def make():
    x = 1

    def inner():
        return x

    return inner


func = make()
print("type(func)", type(func))
print("type(func).__name__", getattr(type(func), "__name__", None))
print("has __closure__", hasattr(func, "__closure__"))
try:
    print("__closure__", func.__closure__)
except Exception as exc:
    print("__closure__ error", type(exc).__name__, exc)
print("getattr", getattr(func, "__closure__", "<missing>"))
print("types.CellType", types.CellType)
