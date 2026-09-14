"""Dynamic reads are callback boundaries, even when their results are unused."""

events = []
observed = []


class ReadCallbacks:
    def __len__(self):
        events.append("len")
        observed.append(1)
        return len(observed)

    def __getitem__(self, key):
        events.append("getitem")
        observed.append(key)
        return len(observed)

    def __getattr__(self, name):
        events.append(name)
        observed.append(1)
        if name == "missing":
            raise AttributeError(name)
        if name == "broken":
            raise ValueError(name)
        return len(observed)


class InstanceCallbacks(type):
    def __instancecheck__(cls, value):
        events.append("instancecheck")
        observed.append(1)
        return len(observed) % 2 == 0


class Checked(metaclass=InstanceCallbacks):
    pass


class IndexCallback:
    def __index__(self):
        events.append("index")
        observed.append(1)
        return 0


class HashCallback:
    def __hash__(self):
        events.append("hash")
        observed.append(1)
        return 7

    def __eq__(self, other):
        events.append("eq")
        observed.append(1)
        return True


def report(label, before, result):
    print(label, before, len(observed), result, events)
    events.clear()
    observed.clear()


def exercise():
    obj = ReadCallbacks()

    before = len(observed)
    len(obj)
    len(obj)
    result = len(obj)
    report("len", before, result)

    before = len(observed)
    obj[0]
    obj[0]
    result = obj[0]
    report("getitem", before, result)

    before = len(observed)
    obj.value
    obj.value
    result = obj.value
    report("getattr", before, result)

    before = len(observed)
    hasattr(obj, "missing")
    hasattr(obj, "missing")
    result = hasattr(obj, "missing")
    report("hasattr-missing", before, result)

    before = len(observed)
    try:
        hasattr(obj, "broken")
    except ValueError as error:
        print("hasattr-error", str(error))
    report("hasattr-raising", before, None)

    before = len(observed)
    isinstance(obj, Checked)
    isinstance(obj, Checked)
    result = isinstance(obj, Checked)
    report("isinstance", before, result)

    sequence = [17]
    index = IndexCallback()
    before = len(observed)
    sequence[index]
    sequence[index]
    result = sequence[index]
    report("list-index", before, result)

    mapping = {HashCallback(): 23}
    key = HashCallback()
    events.clear()
    observed.clear()
    before = len(observed)
    mapping[key]
    mapping[key]
    result = mapping[key]
    report("dict-index", before, result)


exercise()
