"""Purpose: differential coverage for count attr."""


def show(label, value):
    print(label, value)


s = "balloon"
s_count = s.count
show("str_attr_count", s_count("l"))
show("str_attr_count_range", s_count("l", 2, 6))
show("str_attr_count_empty", s_count("", 1, 3))

b = b"ababa"
b_count = b.count
show("bytes_attr_count", b_count(b"a"))
show("bytes_attr_count_range", b_count(b"ba", 1, 4))

ba = bytearray(b"ababa")
ba_count = ba.count
show("bytearray_attr_count", ba_count(b"a"))
show("bytearray_attr_count_range", ba_count(b"ba", 1, 4))
