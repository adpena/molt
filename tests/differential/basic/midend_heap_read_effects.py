"""Repeated heap reads observe writes through calls, aliases, callbacks and handlers."""


class Box:
    def __init__(self, value):
        self.value = value


class Truthy:
    def __init__(self, log):
        self.log = log

    def __bool__(self):
        self.log.append("bool")
        return len(self.log) % 2 == 1


class Adder:
    def __init__(self, box):
        self.box = box

    def __add__(self, other):
        self.box.value += other
        return self.box.value


def mutate(box, items, table):
    box.value += 1
    items.append(len(items))
    table["k"] = table.get("k", 0) + 10


def reads_around_call(box, items, table):
    before = (box.value, len(items), items[-1], table["k"])
    mutate(box, items, table)
    after = (box.value, len(items), items[-1], table["k"])
    print("call", before, after, before == after)


def reads_through_alias(items):
    alias = items
    first = items[0]
    alias[0] = "changed"
    second = items[0]
    print("alias", first, second, first is second)


def reads_around_callbacks():
    log = []
    flag = Truthy(log)
    box = Box(5)
    adder = Adder(box)
    seen = []
    for _ in range(3):
        start = box.value
        if flag:
            seen.append(("t", start, adder + 2, box.value))
        else:
            seen.append(("f", start, box.value))
    print("callbacks", seen, log)


def reads_across_exception(box):
    values = [box.value]
    try:
        box.value = box.value * 3
        values.append(box.value)
        raise KeyError(box.value)
    except KeyError as error:
        values.append(box.value)
        values.append(error.args[0])
        box.value = -1
    finally:
        values.append(box.value)
    print("exception", values)


def missing_reads(table):
    results = []
    for key in ("present", "absent", "present"):
        try:
            results.append(table[key])
        except KeyError:
            table[key] = len(results)
            results.append(("inserted", key))
    print("missing", results, sorted(table.items()))


def loop_reads(box):
    total = 0
    for index in range(5):
        total += box.value
        if index == 2:
            box.value = 100
    print("loop", total, box.value)


reads_around_call(Box(1), [0], {"k": 1})
reads_through_alias(["original", 2])
reads_around_callbacks()
reads_across_exception(Box(7))
missing_reads({"present": "p"})
loop_reads(Box(3))
