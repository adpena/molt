class A:
    __slots__ = ("x",)


obj = A()
obj.x = 1

try:
    obj.y = 2
    print("plain slots", "no error")
except AttributeError as e:
    print("plain slots", type(e).__name__, str(e))
