"""Purpose: differential coverage for comprehensions."""

print([x * 2 for x in range(3)])
print(sorted({x for x in [1, 2, 2, 3]}))
print({k: v for k, v in [("a", 1), ("b", 2), ("a", 3)]})
print(sorted({x * 3 for x in range(6) if x % 2}))
print({k: v * 10 for k, v in [("a", 1), ("b", 2), ("a", 3)] if v >= 2})
print(sorted({a + b for (a, b) in [(1, 2), (3, 4), (5, 6)] if a < 5}))
print({a: b for (a, b) in [(1, "x"), (2, "y")]})
print([a + b for (a, b) in [(1, 2), (3, 4)]])
print([i * j for i in range(3) for j in range(2) if j])

x = "outer"
vals = [x for x in [1, 2, 3]]
print(vals)
print(x)

y = "set-outer"
set_vals = {y for y in [4, 5]}
print(sorted(set_vals))
print(y)

z = "dict-outer"
dict_vals = {z: z + 1 for z in [7, 8]}
print(dict_vals)
print(z)

tuple_late_funcs = [lambda: a for (a, b) in [(1, 2), (3, 4)]]
print([fn() for fn in tuple_late_funcs])
tuple_direct_late = [
    (a, fn()) for a, fn in [(a, lambda: a) for (a, b) in [(1, 2), (3, 4)]]
]
print(tuple_direct_late)

set_late_funcs = list({lambda: s for s in range(3)})
print(sorted(fn() for fn in set_late_funcs))

dict_late_funcs = {d: (lambda: d) for d in range(3)}
print([dict_late_funcs[key]() for key in sorted(dict_late_funcs)])


def dict_tuple_target_shadow():
    field_tuple = ("x", "y")
    field_index = {name: idx for idx, name in enumerate(field_tuple)}
    print("field_index", field_index)
    for name, value in [("z", 3)]:
        pass
    print("after-field-index", name, value)


dict_tuple_target_shadow()


def explode():
    print("explode-called")
    return 1 // 0


lazy_probe = (explode() for _ in range(1))
print("genexpr-created")
try:
    next(lazy_probe)
except ZeroDivisionError:
    print("genexpr-raised-on-next")


def make_gen():
    x = 1
    gen = (x for _ in range(2))
    x = 5
    return list(gen)


print(make_gen())

x = 1
gen = (x for _ in range(2))
x = 7
print(list(gen))

x = "scope"
_ = [x for x in range(2)]
print(x)
