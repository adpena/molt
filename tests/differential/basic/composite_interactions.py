"""Purpose: differential coverage for composite interactions."""


def section(name):
    print(f"--- {name} ---")


class Accumulator:
    def __init__(self, start=0, **meta):
        self.total = start
        self.meta = dict(meta)

    def add(self, *values, scale=1, **meta):
        for val in values:
            self.total += val * scale
        for key, value in meta.items():
            self.meta[key] = value
        return self.total

    def snapshot(self, prefix=""):
        keys = sorted(self.meta)
        parts = []
        for key in keys:
            parts.append(f"{key}={self.meta[key]}")
        return f"{prefix}{self.total}|{','.join(parts)}"


class Boom(Exception):
    pass


def make_scaler(factor):
    def apply(value):
        return value * factor

    return apply


def emit_records(tag, *values, scale=1):
    for idx, val in enumerate(values):
        if val is None:
            yield f"{tag}:{idx}:none"
            continue
        yield f"{tag}:{idx}:{val * scale}"


def might_fail(flag, **meta):
    if flag:
        raise Boom("boom", meta.get("code", 0))
    return meta.get("code", 0)


section("Classes and kwargs")
acc = Accumulator(1, label="base")
print(acc.add(2, 3, scale=2, tag="run1"))
print(acc.snapshot("acc="))
args = (4, 5)
kwargs = {"scale": 3, "note": "x"}
print(acc.add(*args, **kwargs))
print(acc.snapshot("acc="))

section("Closures and calls")
scale_by = make_scaler(3)
print(scale_by(5))

section("Generator and comprehension")
gen = emit_records("g", 1, None, 3, scale=10)
records = [item for item in gen]
print(records)

section("Containers and ordering")
data = {"b": 2, "a": 1, "c": 3}
pairs = []
for key in sorted(data):
    pairs.append(f"{key}:{data[key]}")
print(pairs)
odds = [val * val for val in data.values() if val % 2 == 1]
odds.sort()
print(odds)

section("Exceptions and kwargs")
try:
    might_fail(True, code=7)
except Boom as exc:
    print("boom", exc.args)
print("ok", might_fail(False, code=2))
