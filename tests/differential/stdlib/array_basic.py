"""Purpose: differential coverage for array basic."""

from array import array, typecodes

# typecodes string contains expected codes
print("has_i", "i" in typecodes)
print("has_d", "d" in typecodes)
print("has_f", "f" in typecodes)
print("has_h", "h" in typecodes)

# integer array: init, len, getitem, append, tolist
a = array("i", [1, 2, 3])
print("len_i", len(a))
print("get_0", a[0])
print("get_2", a[2])
a.append(4)
print("after_append", a.tolist())
print("len_after", len(a))

# setitem
a[0] = 10
print("after_set", a.tolist())

# pop
v = a.pop()
print("popped", v)
print("after_pop", a.tolist())

# insert
a.insert(1, 99)
print("after_insert", a.tolist())

# remove
a.remove(99)
print("after_remove", a.tolist())

# count
a2 = array("i", [1, 2, 1, 3, 1])
print("count_1", a2.count(1))
print("count_5", a2.count(5))

# index
print("index_2", a2.index(2))

# reverse
a3 = array("i", [1, 2, 3])
a3.reverse()
print("reversed", a3.tolist())

# extend
a4 = array("i", [1, 2])
a4.extend([3, 4])
print("extended", a4.tolist())

# repeat / in-place repeat
a_repeat = array("i", [7, 8])
print("repeat_3", (a_repeat * 3).tolist())
a_repeat *= 2
print("irepeat_2", a_repeat.tolist())
print("repeat_0", (a_repeat * 0).tolist())

# float array
af = array("d", [1.5, 2.5, 3.5])
print("float_len", len(af))
print("float_0", af[0])
print("float_list", af.tolist())

# slice get/set
a_slice = array("i", [1, 2, 3, 4])
print("slice_basic", a_slice[1:3].tolist())
print("slice_step", a_slice[::2].tolist())
a_slice[1:3] = array("i", [9, 10])
print("slice_assign", a_slice.tolist())
a_slice[::2] = array("i", [7, 8])
print("slice_assign_step", a_slice.tolist())

# mutator returns
a_mut_ret = array("i", [1, 2])
print("append_ret", a_mut_ret.append(3))
print("setitem_ret", a_mut_ret.__setitem__(0, 9))

# deletion
a_del = array("i", [1, 2, 3, 4, 5])
print("del_index_ret", a_del.__delitem__(1))
print("after_del_index", a_del.tolist())
a_del.__delitem__(-1)
print("after_del_negative", a_del.tolist())

a_del_slice = array("i", [1, 2, 3, 4, 5])
del a_del_slice[1:4]
print("after_del_slice", a_del_slice.tolist())

a_del_step = array("i", [1, 2, 3, 4, 5, 6])
del a_del_step[::2]
print("after_del_step", a_del_step.tolist())

a_del_reverse_step = array("i", [1, 2, 3, 4, 5, 6])
del a_del_reverse_step[5:0:-2]
print("after_del_reverse_step", a_del_reverse_step.tolist())

a_del_empty_slice = array("i", [1, 2, 3])
del a_del_empty_slice[2:1]
print("after_del_empty_slice", a_del_empty_slice.tolist())

# tobytes / frombytes round-trip
a5 = array("i", [10, 20, 30])
raw = a5.tobytes()
print("tobytes_type", type(raw).__name__)
a6 = array("i")
a6.frombytes(raw)
print("frombytes_list", a6.tolist())

# typecode and itemsize properties
a7 = array("i", [1])
print("typecode", a7.typecode)
print("itemsize_positive", a7.itemsize > 0)

# empty array
a_empty = array("i")
print("empty_len", len(a_empty))
print("empty_list", a_empty.tolist())

# error: bad typecode
try:
    array("z", [1])
except ValueError:
    print("bad_typecode", "ValueError")

# error: index out of range (via index method)
try:
    a_empty.index(42)
except ValueError:
    print("index_empty_err", "ValueError")

# error: slice assignment requires array
try:
    a_slice[1:3] = [1, 2]
except TypeError as exc:
    print("slice_assign_list_err", type(exc).__name__, str(exc))

# error: deletion index out of range
try:
    a_del_error = array("i", [1, 2, 3])
    del a_del_error[5]
except IndexError as exc:
    print("del_oob_err", type(exc).__name__, str(exc))

# error: deletion index must be an integer
try:
    a_del_error = array("i", [1, 2, 3])
    a_del_error.__delitem__("x")
except TypeError as exc:
    print("del_str_err", type(exc).__name__, str(exc))

# error: deletion slice step cannot be zero
try:
    a_del_error = array("i", [1, 2, 3])
    del a_del_error[::0]
except ValueError as exc:
    print("del_zero_step_err", type(exc).__name__, str(exc))
