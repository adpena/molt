"""Purpose: differential coverage for nested comprehension capture of outer vars."""

vals = [[i + j for j in range(2)] for i in range(2)]
print("vals", vals)

funcs = [(lambda: i) for i in range(3)]
print("late", [f() for f in funcs])

# Outer acquisition is eager, whereas nested iterables and captured bindings
# are read when iteration reaches them. Namespace callbacks cannot redirect the
# hidden frame parameter, even when user names resemble compiler temporaries.
events = []


class Values:
    def __iter__(self):
        events.append("iter")
        return iter((2, 3))


globals()["source"] = Values()
__molt_genexpr_outer_iter_1 = "user binding"
gen = (x for x in source)
source = ()
print("acquired", events)
print("consumed", list(gen), events, __molt_genexpr_outer_iter_1)


def nested():
    first = Values()
    offset = 10
    inner = [1]
    pending = (x + y + offset for x in first for y in inner)
    offset = 20
    inner = [4, 5]
    return pending


print("nested", list(nested()))


class Owner:
    source = (4, 5)
    pending = (value for value in source)


print("class", list(Owner.pending))
