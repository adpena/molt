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
