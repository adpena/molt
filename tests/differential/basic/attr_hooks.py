"""Purpose: differential coverage for attr hooks."""

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


print("dyn_call_list")
lst = [1, 2, 3]
get_append = getattr(lst, "append")
get_append(4)
get_pop = getattr(lst, "pop")
print(get_pop())
print(lst)

print("dyn_call_dict")
data = {"a": 1}
get_get = getattr(data, "get")
print(get_get("a"), get_get("missing", 7))

print("dyn_call_str")
text = "banana"
get_replace = getattr(text, "replace")
print(get_replace("a", "o"))
print(get_replace("a", "o", 1))


def deco(fn):
    def wrapper(*args, **kwargs):
        return ("wrap", fn(*args, **kwargs))

    return wrapper


class WrapObj:
    def __init__(self, fn):
        self.fn = fn

    def __call__(self, *args, **kwargs):
        return ("obj", self.fn(*args, **kwargs))


def deco_obj(fn):
    return WrapObj(fn)


@deco
def add(x, y=1):
    return x + y


@deco_obj
def mul(x, y=2):
    return x * y


print(add(2))
print(add(2, 4))
print(mul(3))
print(mul(3, 5))
