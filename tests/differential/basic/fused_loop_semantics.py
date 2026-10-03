"""Purpose: fused loops and statements behave exactly as the code they replace.

Each function below has a shape the frontend may run in runtime kernels:
``for w in line.split([sep]): d[w] = d.get(w, 0) + delta`` (whole lines of a
bounded number of words), the statement ``d[k] = d.get(k, 0) + delta``, a
counted ``while i < N: ...; i += 1`` and a counted ``buf[i] = FILL`` loop (in
bounded chunks, the ordinary loop finishing from the published index). A
kernel may only stand in where the code provably runs no Python code;
everywhere else (subclasses, callbacks, non-int values, errors, odd
whitespace) the ordinary lowering must produce CPython's results, side
effects, final bindings and exceptions.
"""

import sys
from dataclasses import dataclass, field

events = []


def show(label, value):
    print(label, value, events[:])
    events.clear()


# --- for w in line.split([sep]): d[w] = d.get(w, 0) + delta ---


def count_words(line, counts, delta):
    word = "<none>"
    try:
        for word in line.split():
            counts[word] = counts.get(word, 0) + delta
    except Exception as exc:
        return "error", type(exc).__name__, str(exc), word, list(counts.items())
    return word, list(counts.items())


def count_fields(line, counts, delta):
    field_name = "<none>"
    try:
        for field_name in line.split(","):
            counts[field_name] = counts.get(field_name, 0) + delta
    except Exception as exc:
        return "error", type(exc).__name__, str(exc), field_name, list(counts.items())
    return field_name, list(counts.items())


def count_empty_separator(line, counts):
    word = "<none>"
    try:
        for word in line.split(""):
            counts[word] = counts.get(word, 0) + 1
    except ValueError as exc:
        return "ValueError", str(exc), word, list(counts.items())
    return word, list(counts.items())


def count_multichar(line, counts):
    for word in line.split("<>"):
        counts[word] = counts.get(word, 0) + 1
    return word, list(counts.items())


class LoggingDict(dict):
    def __setitem__(self, key, value):
        events.append(("set", key, value))
        super().__setitem__(key, value)


class DefaultingDict(dict):
    def get(self, key, default=None):
        events.append(("get", key, default))
        return super().get(key, 10)


class Words(str):
    def split(self, *args):
        events.append(("split",) + args)
        return ["p", "q", "p"]


class Delta:
    def __radd__(self, other):
        events.append(("radd", other))
        return other + 100


class Sneaky:
    def __init__(self, text):
        self.text = text

    def __hash__(self):
        return hash(self.text)

    def __eq__(self, other):
        events.append(("eq", self.text, other))
        return other == self.text


show("ascii", count_words("a b a", {}, 1))
show("empty line", count_words("", {}, 1))
show("blank line", count_words(" \t\n ", {"x": 1}, 1))
show("edges", count_words("  x \t\n y  ", {}, 1))
show(
    "unicode whitespace",
    count_words("a b a　c\x1cd\x1fe\x85f g​h", {}, 1),
)
show("not whitespace", count_words("a​b﻿c", {}, 1))
show("separator", count_fields("a,b,,a,", {}, 1))
show("separator only", count_fields(",", {}, 1))
show("multichar separator", count_multichar("a<>b<><>a", {}))
show("empty separator", count_empty_separator("a b", {"k": 1}))
show("bool delta", count_words("a a", {}, True))
show("float delta", count_words("a a", {}, 0.5))
show("big delta", count_words("a a", {}, 2**62))
show("negative delta", count_words("a b a", {"a": 1}, -1))
show("float value", count_words("a", {"a": 1.5}, 1))
show("big value", count_words("a", {"a": 2**63 - 1}, 1))
show("str value", count_words("b a c", {"a": "x"}, 1))
show("none value", count_words("a", {"a": None}, 1))
show("int key present", count_words("1 1", {1: 5}, 1))
show("setitem override", count_words("a b a", LoggingDict(), 1))
show("get override", count_words("a b a", DefaultingDict(), 1))
show("split override", count_words(Words("ignored"), {}, 1))
show("radd delta", count_words("a b", {}, Delta()))
sneaky = Sneaky("a")
result = count_words("a b", {sneaky: 5}, 1)
show("same-hash key", (result[0], [(getattr(k, "text", k), v) for k, v in result[1]]))


# --- d[k] = d.get(k, 0) + delta ---


def bump(counts, key, delta):
    try:
        counts[key] = counts.get(key, 0) + delta
    except Exception as exc:
        return "error", type(exc).__name__, str(exc), list(counts.items())
    return list(counts.items())


def bump_twice(counts, key):
    counts[key] = counts.get(key, 0) + 1
    counts[key] = counts.get(key, 0) + 1
    return list(counts.items())


show("bump new", bump({}, "a", 1))
show("bump existing", bump({"a": 41}, "a", 1))
show("bump bool", bump({"a": True}, "a", True))
show("bump across i64", bump({"a": 2**63 - 1}, "a", 1))
show("bump big delta", bump({}, "a", -(2**70)))
show("bump float", bump({"a": 1}, "a", 0.25))
show("bump int key", bump({}, 7, 1))
show("bump none value", bump({"a": None}, "a", 1))
show("bump str value", bump({"a": "x"}, "a", 1))
show("bump setitem override", bump(LoggingDict(a=1), "a", 1))
show("bump get override", bump(DefaultingDict(), "a", 1))
show("bump radd", bump({}, "a", Delta()))
show("bump same-hash key", [(k.text, v) for k, v in bump({sneaky: 1}, "a", 1)])
show("bump twice", bump_twice({}, "z"))

MODULE_COUNTS = {}
MODULE_KEY = "m"
MODULE_COUNTS[MODULE_KEY] = MODULE_COUNTS.get(MODULE_KEY, 0) + 1
MODULE_COUNTS[MODULE_KEY] = MODULE_COUNTS.get(MODULE_KEY, 0) + 1
show("module bump", list(MODULE_COUNTS.items()))


@dataclass
class Order:
    region: str
    note: str = field(init=False)


class LookupLoggingDict(dict):
    def __getattribute__(self, name):
        events.append(("lookup", name))
        return super().__getattribute__(name)


def total_by_region(regions, totals):
    for region in regions:
        order = Order(region)
        totals[order.region] = totals.get(order.region, 0) + 1
    return list(totals.items())


def total_by_unset_field(totals):
    order = Order("NA")
    try:
        totals[order.note] = totals.get(order.note, 0) + 1
    except AttributeError as exc:
        return "AttributeError", str(exc)
    return list(totals.items())


show("dataclass key", total_by_region(["NA", "EU", "NA"], {}))
show("dataclass key subclass", total_by_region(["NA"], LoggingDict()))
show("unset field, exact dict", total_by_unset_field({}))
# ``totals.get`` is looked up before ``order.note`` raises.
show("unset field, logging dict", total_by_unset_field(LookupLoggingDict()))


# --- while i < N: ...; i += 1 ---


class LoudInt(int):
    def __radd__(self, other):
        events.append(("LoudInt.__radd__", other, int(self)))
        return other + int(self)


def count_up(start):
    i = start
    total = 0
    while i < 5:
        total += i
        i += 1
    return total, i, type(i).__name__


def count_across_i64():
    i = 9223372036854775806
    steps = 0
    while i < 9223372036854775809:
        steps += 1
        i += 1
    return steps, i


def count_rebinding_through_frame():
    i = 0
    seen = []

    def callback():
        frame = sys._getframe(1)
        if frame.f_locals["i"] == 2:
            frame.f_locals["i"] = 10

    while i < 5:
        seen.append(i)
        callback()
        i += 1
    return seen, i


show("counted int", count_up(0))
show("counted no iterations", count_up(7))
show("counted float start", count_up(0.5))
show("counted bool start", count_up(True))
show("counted big start", count_up(2**70))
show("counted negative start", count_up(-3))
show("counted subclass start", count_up(LoudInt(3)))
show("counted across i64", count_across_i64())
# From 3.13 the callback rebinds ``i`` through the frame proxy (PEP 667).
show("counted frame rebinding", count_rebinding_through_frame())


# --- while i < N: buf[i] = FILL; i += 1 ---


class LoggingBytes(bytearray):
    def __setitem__(self, index, value):
        events.append(("setitem", index, value))
        super().__setitem__(index, value)


def fill_new():
    buf = bytearray(4)
    i = 0
    while i < 4:
        buf[i] = 97
        i += 1
    return bytes(buf), i


def fill_negative_start():
    buf = bytearray(4)
    i = -2
    while i < 2:
        buf[i] = 97
        i += 1
    return bytes(buf), i


def fill_short():
    buf = bytearray(2)
    i = 0
    try:
        while i < 4:
            buf[i] = 97
            i += 1
    except IndexError as exc:
        return "IndexError", str(exc), bytes(buf), i
    return bytes(buf), i


def fill_subclass():
    buf = LoggingBytes(3)
    i = 0
    while i < 3:
        buf[i] = 98
        i += 1
    return bytes(buf), i


def fill_float_index():
    buf = bytearray(4)
    i = 0.0
    try:
        while i < 4:
            buf[i] = 97
            i += 1
    except TypeError as exc:
        return "TypeError", str(exc), bytes(buf), i
    return bytes(buf), i


show("fill", fill_new())
show("fill negative start", fill_negative_start())
show("fill past the end", fill_short())
show("fill subclass", fill_subclass())
show("fill float index", fill_float_index())


# A fill longer than one chunk leaves the index at the bound; one whose buffer
# is shorter than the bound runs the ordinary loop, which raises at the first
# missing index.
def fill_many():
    buf = bytearray(3_000_000)
    i = 0
    while i < 3_000_000:
        buf[i] = 7
        i += 1
    return buf[0], buf[1_048_576], buf[-1], buf.count(7), i


def fill_many_short():
    buf = bytearray(2_500_000)
    i = 0
    try:
        while i < 3_000_000:
            buf[i] = 7
            i += 1
    except IndexError as exc:
        return "IndexError", str(exc), buf.count(7), i
    return buf.count(7), i


show("fill many", fill_many())
show("fill many short", fill_many_short())

# A line with more words than one bounded split runs the ordinary loop.
show("many words", count_words(" ".join(["w", "x"] * 3000), {}, 1))
