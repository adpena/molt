"""Purpose: differential coverage for range ops."""

print(len(range(5)))
print(range(5)[0])
print(range(5)[-1])

total = 0
for i in range(3):
    total = total + i
print(total)

acc = 0
for i in range(5, 0, -2):
    acc = acc + i
print(acc)

r = range(5, 0, -2)
print(len(r))
print(r[0])
print(r[1])

lst = list(range(1, 6, 2))
print(len(lst))
print(lst[0])
print(lst[-1])


class Indexable:
    def __index__(self):
        return 4


print(list(range(Indexable())))
total = 0
for i in range(Indexable()):
    total += i
print(total)

try:
    list(range(1, 3, 0))
    print("range-step-zero-list-ok")
except ValueError:
    print("range-step-zero-list-valueerror")

try:
    for _ in range(1, 3, 0):
        pass
    print("range-step-zero-loop-ok")
except ValueError:
    print("range-step-zero-loop-valueerror")


# CPython 3.12.13 Objects/rangeobject.c::compute_slice uses exact long
# normalization, not Py_ssize_t clamping or element enumeration.
for value in (
    range(2, 29, 3),
    range(29, 2, -3),
    range(4, 4, -7),
    range(-(10**80), 10**90, 10**30 + 1),
):
    for key in (
        slice(None),
        slice(1, None),
        slice(None, None, -1),
        slice(-20, 20, 2),
        slice(20, -20, -3),
        slice(-(10**100), 10**100, 10**50 + 3),
        slice(10**100, -(10**100), -(10**50) - 3),
    ):
        print("range source slice", value[key])
        print("range base slice", range.__getitem__(value, key))


slice_events = []
slice_error = ValueError("range slice conversion")


class RangeSliceBound:
    def __init__(self, label, value, failure=False):
        self.label = label
        self.value = value
        self.failure = failure

    def __index__(self):
        slice_events.append(self.label)
        if self.failure:
            raise slice_error
        return self.value


for failing in (None, "step", "start", "stop", "zero", "nonint"):
    for unbound in (False, True):
        slice_events.clear()
        key = slice(
            RangeSliceBound("start", 1, failing == "start"),
            RangeSliceBound("stop", 8, failing == "stop"),
            RangeSliceBound(
                "step", 0 if failing == "zero" else ("bad" if failing == "nonint" else 2),
                failing == "step",
            ),
        )
        try:
            source = range(1, 30, 3)
            result = range.__getitem__(source, key) if unbound else source[key]
            print("range slice coercion", failing, unbound, result, slice_events)
        except Exception as exc:
            print(
                "range slice error", failing, unbound, type(exc).__name__, str(exc),
                exc is slice_error, slice_events,
            )
