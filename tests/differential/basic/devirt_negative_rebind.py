"""Rebindable module names must keep late-binding call semantics."""


def base_double(x):
    return x * 2


def base_triple(x):
    return x * 3


f = base_double
f = base_triple
print(f(2))

flag = True
if flag:
    picked = base_double
else:
    picked = base_triple
print(picked(10))

g = base_double
print(g(4))
g = base_triple
print(g(4))


def use_rebound():
    h = f
    return h(5)


print(use_rebound())
