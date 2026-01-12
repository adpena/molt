calls = []


class C:
    def __getattr__(self, name):
        return f"missing:{name}"


class D:
    def __setattr__(self, name, value):
        calls.append((name, value))


c = C()
print(c.foo)
print(getattr(c, "bar", "default"))
c.ok = 5
print(c.ok)

d = D()
d.x = 7
print(calls)
try:
    _ = d.x
except AttributeError:
    print("noattr")
