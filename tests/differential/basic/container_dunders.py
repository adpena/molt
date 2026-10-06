"""Purpose: differential coverage for container dunders."""


def show(label, value):
    print(label, value)


d = {"b": 2, "a": 1}
show("d_iter", list(d.__iter__()))
show("d_len", d.__len__())
show("d_contains_a", d.__contains__("a"))
show("d_contains_z", d.__contains__("z"))
show("d_reversed", list(d.__reversed__()))
show("d_type_iter", list(dict.__iter__(d)))
show("d_type_len", dict.__len__(d))
show("d_type_contains_a", dict.__contains__(d, "a"))
show("d_type_contains_z", dict.__contains__(d, "z"))
show("d_type_reversed", list(dict.__reversed__(d)))
try:
    show("d_contains_unhashable", d.__contains__([]))
except TypeError as exc:
    show("d_contains_unhashable_error", type(exc).__name__)
try:
    show("d_type_contains_unhashable", dict.__contains__(d, []))
except TypeError as exc:
    show("d_type_contains_unhashable_error", type(exc).__name__)

d_empty = {}
show("d_empty_iter", list(d_empty.__iter__()))
show("d_empty_len", d_empty.__len__())
show("d_empty_contains_a", d_empty.__contains__("a"))
show("d_empty_reversed", list(d_empty.__reversed__()))
show("d_empty_type_iter", list(dict.__iter__(d_empty)))
show("d_empty_type_len", dict.__len__(d_empty))
show("d_empty_type_contains_a", dict.__contains__(d_empty, "a"))
show("d_empty_type_reversed", list(dict.__reversed__(d_empty)))

lst = [1, 2, 3]
show("l_iter", list(lst.__iter__()))
show("l_len", lst.__len__())
show("l_contains_2", lst.__contains__(2))
show("l_contains_9", lst.__contains__(9))
show("l_reversed", list(lst.__reversed__()))
show("l_type_iter", list(list.__iter__(lst)))
show("l_type_len", list.__len__(lst))
show("l_type_contains_2", list.__contains__(lst, 2))
show("l_type_contains_9", list.__contains__(lst, 9))
show("l_type_reversed", list(list.__reversed__(lst)))

lst_empty = []
show("l_empty_iter", list(lst_empty.__iter__()))
show("l_empty_len", lst_empty.__len__())
show("l_empty_contains_2", lst_empty.__contains__(2))
show("l_empty_reversed", list(lst_empty.__reversed__()))
show("l_empty_type_iter", list(list.__iter__(lst_empty)))
show("l_empty_type_len", list.__len__(lst_empty))
show("l_empty_type_contains_2", list.__contains__(lst_empty, 2))
show("l_empty_type_reversed", list(list.__reversed__(lst_empty)))

s = "hi"
show("s_iter", list(s.__iter__()))
show("s_len", s.__len__())
show("s_contains_h", s.__contains__("h"))
show("s_contains_hi", s.__contains__("hi"))
show("s_contains_x", s.__contains__("x"))
show("s_type_iter", list(str.__iter__(s)))
show("s_type_len", str.__len__(s))
show("s_type_contains_h", str.__contains__(s, "h"))
show("s_type_contains_hi", str.__contains__(s, "hi"))
show("s_type_contains_x", str.__contains__(s, "x"))
try:
    show("s_contains_int", s.__contains__(1))
except TypeError as exc:
    show("s_contains_int_error", type(exc).__name__)
try:
    show("s_type_contains_int", str.__contains__(s, 1))
except TypeError as exc:
    show("s_type_contains_int_error", type(exc).__name__)
try:
    show("s_reversed", list(s.__reversed__()))
except AttributeError as exc:
    show("s_reversed_error", type(exc).__name__)
try:
    show("s_type_reversed", list(str.__reversed__(s)))
except AttributeError as exc:
    show("s_type_reversed_error", type(exc).__name__)

s_empty = ""
show("s_empty_iter", list(s_empty.__iter__()))
show("s_empty_len", s_empty.__len__())
show("s_empty_contains_empty", s_empty.__contains__(""))
show("s_empty_contains_x", s_empty.__contains__("x"))
show("s_empty_type_iter", list(str.__iter__(s_empty)))
show("s_empty_type_len", str.__len__(s_empty))
show("s_empty_type_contains_empty", str.__contains__(s_empty, ""))
show("s_empty_type_contains_x", str.__contains__(s_empty, "x"))
try:
    show("s_empty_reversed", list(s_empty.__reversed__()))
except AttributeError as exc:
    show("s_empty_reversed_error", type(exc).__name__)
try:
    show("s_empty_type_reversed", list(str.__reversed__(s_empty)))
except AttributeError as exc:
    show("s_empty_type_reversed_error", type(exc).__name__)

b = b"ab"
show("b_iter", list(b.__iter__()))
show("b_len", b.__len__())

ba = bytearray(b"ab")
show("ba_iter", list(ba.__iter__()))
show("ba_len", ba.__len__())


class ContainsIter:
    def __contains__(self, item):
        print("c_contains", item)
        return item == "hit"

    def __iter__(self):
        print("c_iter")
        return iter(["hit", "miss"])


ci = ContainsIter()
show("c_in_hit", "hit" in ci)
show("c_in_miss", "miss" in ci)


class IterFallback:
    def __iter__(self):
        print("i_iter")
        return iter([1, 2, 3])


it = IterFallback()
show("i_in_2", 2 in it)
show("i_in_9", 9 in it)


class GetItemFallback:
    def __init__(self):
        self.data = [10, 20]

    def __getitem__(self, idx):
        print("g_getitem", idx)
        return self.data[idx]


gi = GetItemFallback()
show("g_in_20", 20 in gi)
show("g_in_99", 99 in gi)

# Source multiplication must evaluate operands once in source order, run the
# numeric/reflected protocol, and only then ask for an index-sized count.
repeat_events = []


class RepeatCount:
    def __init__(self, number, mode="index"):
        self.number = number
        self.mode = mode

    def __mul__(self, other):
        repeat_events.append("mul")
        if self.mode == "value":
            return "numeric left"
        if self.mode == "raise":
            raise RuntimeError("multiply")
        return NotImplemented

    def __rmul__(self, other):
        repeat_events.append("rmul")
        if self.mode == "value":
            return "numeric right"
        if self.mode == "raise":
            raise RuntimeError("multiply")
        return NotImplemented

    def __index__(self):
        repeat_events.append("index")
        if self.mode == "index-raise":
            raise ValueError("count")
        return self.number


class RepeatInt(int):
    def __index__(self):
        raise AssertionError("an int subclass uses its integer payload")

    def __rmul__(self, other):
        repeat_events.append("int-rmul")
        return NotImplemented

    def __mul__(self, other):
        repeat_events.append("int-mul")
        return NotImplemented


def repeat_operand(label, value):
    repeat_events.append(label)
    return value


for sequence in ([7], (7,), "x", b"x", bytearray(b"x")):
    for mode in ("index", "value", "raise", "index-raise"):
        for reverse in (False, True):
            repeat_events.clear()
            count = RepeatCount(2, mode)
            try:
                if reverse:
                    result = repeat_operand("left", count) * repeat_operand("right", sequence)
                else:
                    result = repeat_operand("left", sequence) * repeat_operand("right", count)
                print("repeat protocol", type(sequence).__name__, mode, reverse, result, repeat_events)
            except (RuntimeError, ValueError) as error:
                print("repeat protocol", type(sequence).__name__, mode, reverse,
                      type(error).__name__, str(error), repeat_events)


for count in (False, True, -2, 0, RepeatInt(2), 2.0, 2**100, -(2**100)):
    for reverse in (False, True):
        repeat_events.clear()
        try:
            result = count * [7] if reverse else [7] * count
            print("repeat count", type(count).__name__, reverse, len(result), result, repeat_events)
        except (TypeError, OverflowError) as error:
            print("repeat count", type(count).__name__, reverse, type(error).__name__, repeat_events)


for mode in ("index", "value"):
    repeat_events.clear()
    values = [7]
    values *= RepeatCount(2, mode)
    print("repeat inplace", mode, values, repeat_events)


# Explicit descriptors bypass reflected numeric dispatch. Mutation, aliasing,
# exceptions, and index-callback reentry follow the same sequence repeat slot.
class RepeatList(list):
    pass


for factory in (list, RepeatList):
    for bound in (False, True):
        for mode in ("value", "raise", "index-raise"):
            repeat_events.clear()
            values = factory([7])
            count = RepeatCount(2, mode)
            try:
                result = values.__imul__(count) if bound else list.__imul__(values, count)
                print("repeat descriptor", factory.__name__, bound, mode,
                      result, result is values, values, repeat_events)
            except ValueError as error:
                print("repeat descriptor", factory.__name__, bound, mode,
                      type(error).__name__, str(error), values, repeat_events)

for method in (list.__mul__, list.__rmul__):
    repeat_events.clear()
    values = [7]
    result = method(values, RepeatCount(2, "raise"))
    print("repeat copy descriptor", result, result is values, values, repeat_events)


class ReentrantRepeatCount:
    def __index__(self):
        repeat_events.append("index")
        values.append(2**100)
        return 2


repeat_events.clear()
values = [7]
result = values.__imul__(ReentrantRepeatCount())
print("repeat descriptor reentry", result, result is values, repeat_events)


# Physical list descriptors and source protocol dispatch share storage but
# differ deliberately in whether subclass special methods participate.
list_slot_events = []


class OverrideList(list):
    def __len__(self):
        return 17

    def __iter__(self):
        return iter(("source iterator",))

    def __getitem__(self, key):
        return ("source get", key)

    def __setitem__(self, key, value):
        list_slot_events.append(("set", key, value))

    def __delitem__(self, key):
        list_slot_events.append(("del", key))

    def __contains__(self, value):
        return value == "source contains"

    def __reversed__(self):
        return iter(("source reversed",))

    def __repr__(self):
        return "source repr"

    def __add__(self, other):
        return "source add"

    def __radd__(self, other):
        return "source radd"

    def __iadd__(self, other):
        list_slot_events.append("iadd")
        return self

    def __imul__(self, other):
        list_slot_events.append("imul")
        return self

    def __eq__(self, other):
        return "source eq"

    def append(self, value):
        list_slot_events.append(("append", value))


def annotated_list_operations(values: list):
    print("subclass source", len(values), bool(values), values[0],
          "source contains" in values, 1 in values, list(iter(values)),
          list(reversed(values)), repr(values))
    values[0] = 12
    del values[1]
    values.append(13)
    values += [14]
    values *= 2
    print("subclass arithmetic", values + [], [] + values, values == [1, 2])
    print("subclass events", list_slot_events)


overrides = OverrideList([1, 2])
annotated_list_operations(overrides)
print("list slots", list.__len__(overrides), list.__getitem__(overrides, 0),
      list.__contains__(overrides, 1), list(list.__iter__(overrides)),
      list(list.__reversed__(overrides)), list.__repr__(overrides))
list.__setitem__(overrides, 0, 3)
list.__delitem__(overrides, 1)
list.append(overrides, 4)
print("list mutating slots", list.__iadd__(overrides, [5]) is overrides,
      list.__imul__(overrides, 2) is overrides, list.__repr__(overrides))
print("list comparisons", list.__eq__(overrides, [3, 4, 5, 3, 4, 5]),
      list.__ne__(overrides, [3]), list.__lt__(overrides, [9]),
      list.__le__(overrides, [9]), list.__gt__(overrides, [0]),
      list.__ge__(overrides, [0]), list.__eq__(overrides, (3,)) is NotImplemented)
print("list copy slots", type(list.__add__(overrides, [])).__name__,
      type(list.__mul__(overrides, 1)).__name__,
      type(list.copy(overrides)).__name__)

for descriptor, args in (
    (list.append, (1,)), (list.extend, ([],)), (list.insert, (0, 1)),
    (list.remove, (1,)), (list.pop, ()), (list.clear, ()), (list.copy, ()),
    (list.reverse, ()), (list.sort, ()), (list.count, (1,)), (list.index, (1,)),
    (list.__init__, ([],)), (list.__len__, ()), (list.__iter__, ()),
    (list.__reversed__, ()), (list.__getitem__, (0,)),
    (list.__setitem__, (0, 1)), (list.__delitem__, (0,)),
    (list.__contains__, (1,)), (list.__add__, ([],)), (list.__mul__, (1,)),
    (list.__iadd__, ([],)), (list.__imul__, (1,)), (list.__repr__, ()),
    (list.__eq__, ([],)), (list.__ne__, ([],)), (list.__lt__, ([],)),
    (list.__le__, ([],)), (list.__gt__, ([],)), (list.__ge__, ([],)),
):
    try:
        descriptor((), *args)
    except TypeError:
        print("list receiver rejected")
    else:
        print("invalid list receiver accepted")


class FalseList(list):
    def __bool__(self):
        return False


print("list bool override", bool(FalseList([1])))


class HashList(list):
    def __hash__(self):
        return 41


print("list hash", list.__hash__ is None, hash(HashList([1])))



exposed_list_storage = None


def expose_list_storage(value):
    # Opaque escape revokes a flat-storage proof without changing exact class.
    global exposed_list_storage
    exposed_list_storage = value


flat_after_escape = [2, 3, 5]
expose_list_storage(flat_after_escape)
print("exact list after escape", flat_after_escape[0], flat_after_escape[-1])
total_after_escape = 0
for exact_index in range(3):
    total_after_escape += flat_after_escape[exact_index]
print("exact list loop after escape", total_after_escape)
copied_after_escape = flat_after_escape.copy()
print("exact list copy storage", copied_after_escape[1])

bool_after_escape = [True, False, True]
expose_list_storage(bool_after_escape)
print("exact bool list after escape", bool_after_escape[0], bool_after_escape[-1])
bool_after_escape.append("promoted")
print("exact bool list after promotion", bool_after_escape[0], bool_after_escape[-1])


only, = overrides
print("list iterable consumers", tuple(overrides), list(overrides), [*overrides], only)
print("list source slice", overrides[:1])


class JoinList(list):
    def __iter__(self):
        return iter(("source", "items"))


class BytesList(list):
    def __iter__(self):
        return iter((65, 66))

    def __bytes__(self):
        return b"custom bytes"


print("list join override", "|".join(JoinList(["storage"])))
print("list bytes overrides", bytes(BytesList([0])), bytearray(BytesList([0])))
