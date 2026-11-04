events = []
revived = []


class Owner:
    def __init__(self, label, revive=False):
        self.label = label
        self.revive = revive

    def __del__(self):
        events.append(self.label)
        # Exercise nested compiled calls during the terminal runtime edge.
        assert sum((1, 2, 3)) == 6
        if self.revive:
            self.revive = False
            revived.append(self)


def wrong_none(value: None):
    alias = value
    return alias


def wrong_float(value: float):
    alias = value
    return alias


def mixed_phi(n):
    value = None
    for i in range(n):
        if i % 4 == 0:
            value = Owner("phi" + str(i))
        elif i % 4 == 1:
            value = 13
        elif i % 4 == 2:
            value = 15
        else:
            value = 17
        alias = value
        box = (alias, alias)
        assert box[0] is box[1]
    return value


def boxed_wide(n):
    x = 1 << 60
    for i in range(n):
        value = x + i
        a = b = value
        pair = (a, b)
        assert pair[0] is pair[1]
        assert pair[0] == (1 << 60) + i
        x = value - i
    return x


def with_error(depth):
    owner = Owner("error" + str(depth))
    try:
        if depth:
            with_error(depth - 1)
        else:
            raise ValueError("leaf")
    finally:
        assert owner.label == "error" + str(depth)


def suspended():
    owner = Owner("generator")
    try:
        yield owner.label
    finally:
        events.append("generator-finally")


def return_owner():
    doomed = Owner("return-doomed")
    result = Owner("return-result")
    assert doomed.label == "return-doomed"
    return result


for wrapper in (wrong_none, wrong_float):
    value = Owner("annotation")
    alias = wrapper(value)
    del value
    assert alias.label == "annotation"
    assert events.count("annotation") == (0 if wrapper is wrong_none else 1)
    del alias
assert events == ["annotation", "annotation"]
assert mixed_phi(12) == 17
assert boxed_wide(31) == 1 << 60
# Full i64 payloads may themselves look like a pointer/None tag. RC must
# classify the physical carrier, then own any BigInt minted by boxing it.
for tag in (0x7FFB, 0x7FFC, 0x7FFD):
    for offset in (0, 1, 4096):
        wide = (tag << 48) + offset
        alias = int(wide)
        pair = (alias, alias)
        assert pair[0] is pair[1]
        assert pair[0] == wide
        assert str(pair[0]) == str((tag << 48) + offset)
g = suspended()
assert next(g) == "generator"
g.close()
del g
value = return_owner()
assert "return-doomed" in events
assert "return-result" not in events
del value
value = Owner("revival", True)
del value
assert len(revived) == 1
assert events.count("revival") == 1
revived.clear()
assert events.count("revival") == 1
print(events)
