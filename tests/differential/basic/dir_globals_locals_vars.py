"""Purpose: differential coverage for dir globals locals vars."""


class C:
    def method(self):
        return 1


print("C" in globals())
print("C" in dir())
print("method" in dir(C))


def f():
    x = 1
    y = 2
    print(sorted(locals().keys()))
    print(vars()["x"], vars()["y"])
    print("x" in globals())


f()

obj = C()
obj.attr = 5
print(vars(obj)["attr"])
