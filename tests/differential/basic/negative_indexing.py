"""Purpose: differential coverage for negative indexing."""


def show(label, value):
    print(label, value)


def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


lst = [1, 2, 3]
show("list_-1", lst[-1])
show("list_-3", lst[-3])
show_err("list_-4", lambda: lst[-4])
lst_mut = [1, 2, 3]
lst_mut[-1] = 9
show("list_set_-1", lst_mut)
del lst_mut[-2]
show("list_del_-2", lst_mut)


def list_set_out_of_range():
    lst_mut[-4] = 1


show_err("list_set_-4", list_set_out_of_range)
lst_attr = [1, 2, 3]
show("list_attr_set_ret", lst_attr.__setitem__(-1, 8))
show("list_attr_set", lst_attr)
show_err("list_attr_set_oob", lambda: lst_attr.__setitem__(-4, 1))
show("list_attr_del_ret", lst_attr.__delitem__(-2))
show("list_attr_del", lst_attr)
show_err("list_attr_del_oob", lambda: lst_attr.__delitem__(-5))

tup = (10, 20, 30)
show("tuple_-1", tup[-1])
show("tuple_-3", tup[-3])
show_err("tuple_-4", lambda: tup[-4])

s = "abc"
show("str_-1", s[-1])
show("str_-3", s[-3])
show_err("str_-4", lambda: s[-4])

b = b"abc"
show("bytes_-1", b[-1])
show("bytes_-3", b[-3])
show_err("bytes_-4", lambda: b[-4])

ba = bytearray(b"abc")
show("bytearray_-1", ba[-1])
show("bytearray_-3", ba[-3])
show_err("bytearray_-4", lambda: ba[-4])
ba_mut = bytearray(b"abc")
ba_mut[-1] = ord("z")
show("bytearray_set_-1", ba_mut)


def bytearray_set_out_of_range():
    ba_mut[-4] = 1


show_err("bytearray_set_-4", bytearray_set_out_of_range)
ba_attr = bytearray(b"abc")
show("bytearray_attr_set_ret", ba_attr.__setitem__(-1, ord("y")))
show("bytearray_attr_set", ba_attr)
show_err("bytearray_attr_set_oob", lambda: ba_attr.__setitem__(-4, 1))
show("bytearray_attr_del_ret", ba_attr.__delitem__(-1))
show("bytearray_attr_del", ba_attr)
show_err("bytearray_attr_del_oob", lambda: ba_attr.__delitem__(-5))

mv = memoryview(b"abc")
show("mv_-1", mv[-1])
show("mv_-3", mv[-3])
show_err("mv_-4", lambda: mv[-4])
show_err("mv_set_ro", lambda: mv.__setitem__(0, 120))
show_err("mv_del_ro", lambda: mv.__delitem__(0))
mv_owner = bytearray(b"abc")
mv_mut = memoryview(mv_owner)
mv_mut[-1] = ord("z")
show("mv_set_-1", mv_owner)


def memoryview_set_out_of_range():
    mv_mut[-4] = 1


show_err("mv_set_-4", memoryview_set_out_of_range)
mv_attr_owner = bytearray(b"abc")
mv_attr = memoryview(mv_attr_owner)
show("mv_attr_set_ret", mv_attr.__setitem__(-1, ord("y")))
show("mv_attr_set", mv_attr_owner)
show_err("mv_attr_set_oob", lambda: mv_attr.__setitem__(-4, 1))
show_err("mv_attr_del", lambda: mv_attr.__delitem__(0))

rng = range(3)
show("range_-1", rng[-1])
show("range_-3", rng[-3])
show_err("range_-4", lambda: rng[-4])
rng_desc = range(5, 0, -2)
show("range_desc_-1", rng_desc[-1])
show("range_desc_-2", rng_desc[-2])
show_err("range_desc_-4", lambda: rng_desc[-4])

d = {"a": 1, "b": 2}
keys = d.keys()
values = d.values()
items = d.items()
show_err("keys_idx", lambda: keys[0])
show_err("keys_neg", lambda: keys[-1])
show_err("values_idx", lambda: values[0])
show_err("values_neg", lambda: values[-1])
show_err("items_idx", lambda: items[0])
show_err("items_neg", lambda: items[-1])
