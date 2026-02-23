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

# float array
af = array("d", [1.5, 2.5, 3.5])
print("float_len", len(af))
print("float_0", af[0])
print("float_list", af.tolist())

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
