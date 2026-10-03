"""Purpose: differential coverage for slice objects."""

lst = [0, 1, 2, 3, 4, 5]
print(lst[1:5:2])
print(lst[::-1])

nums = (1, 2, 3, 4, 5)
print(nums[0:5:2])
print(nums[4:0:-2])

b = b"abcdef"
print(b[1:5:2])
print(b[::-1])

ba = bytearray(b"abcdef")
print(ba[1:5:2])
print(ba[::-1])

s = "abcdef"
print(s[1:5:2])
print(s[::-1])

print(slice(1, 5, 2))
print(slice(None, None, -1))


events = []


class Bound:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append("drop " + self.name)


# A slice stores each bound object itself and keeps it alive until it dies.
bound = Bound("slice")
kept = slice(bound, bound)
del bound
print("slice owners", kept.start is kept.stop, kept.step, events)
del kept
print("slice released", events)
events.clear()

# Omitted bounds are None.
print("omitted bounds", slice(7).start, slice(7).stop, slice(7).step)


def discard_slice():
    slice(None, Bound("discarded"))
    events.append("after discard")


discard_slice()
print("slice discarded", events)
events.clear()


def repeated_wide_bound(steps):
    # A loop-carried int past the inline range may travel unboxed; a bound
    # repeated in one construction must still be one object.
    total = 0
    for _ in range(steps):
        total = total * 3 + 1
    wide = slice(total, total, total + 1)
    return wide.start is wide.stop, wide.start == total, wide.step - wide.stop


print("wide bound identity", repeated_wide_bound(40))
