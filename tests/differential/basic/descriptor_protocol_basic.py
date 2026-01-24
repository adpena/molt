"""Purpose: differential coverage for descriptor protocol basic."""

log: list[tuple] = []


class Desc:
    def __set_name__(self, owner, name):
        log.append(("set_name", owner.__name__, name))

    def __get__(self, obj, owner):
        log.append(("get", obj is None, owner.__name__))
        return 123

    def __set__(self, obj, value):
        log.append(("set", value))

    def __delete__(self, obj):
        log.append(("delete", True))


class C:
    d = Desc()


c = C()
_ = C.d
_ = c.d
c.__dict__["d"] = "shadow"
_ = c.d
c.d = 5
del c.d

print(log)
