def f(a, b=2, /, c=3, *, d, e=5):
    return (a, b, c, d, e)


def g(a, *args, **kwargs):
    return (a, args, (("x", kwargs["x"]), ("y", kwargs["y"])))


print(f(1, d=4))
print(f(1, 2, 3, d=4, e=6))
print(f(*[1, 2, 3], d=4))
print(f(*(1, 2, 3), **{"d": 4}))
print(g(1, 2, 3, x=4, y=5))


def h(a, /, b, *, c):
    return (a, b, c)


try:
    print(h(1, b=2, c=3))
    print("posonly-ok")
except TypeError:
    print("posonly-typeerror")

try:
    h(a=1, b=2, c=3)
    print("posonly-keyword-ok")
except TypeError:
    print("posonly-keyword-typeerror")

try:
    f(1, c=4, **{"c": 5})
    print("dup-kw-ok")
except TypeError:
    print("dup-kw-typeerror")

try:
    f(1, **{1: 2}, d=4)
    print("non-str-kw-ok")
except TypeError:
    print("non-str-kw-typeerror")
