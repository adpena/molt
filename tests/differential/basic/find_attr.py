"""Purpose: differential coverage for find attr."""


def show(label, value):
    print(label, value)


s = "banana"
s_find = s.find
s_rfind = s.rfind
s_starts = s.startswith
s_ends = s.endswith

show("str_find", s_find("na"))
show("str_find_range", s_find("na", 2, 5))
show("str_rfind", s_rfind("na"))
show("str_rfind_range", s_rfind("na", 0, 4))
show("str_starts", s_starts("ba"))
show("str_starts_range", s_starts("na", 2))
show("str_starts_range2", s_starts("na", 2, 4))
show("str_starts_tuple", s_starts(("ba", "na")))
show("str_ends", s_ends("na"))
show("str_ends_range", s_ends("na", 0, 4))
show("str_ends_tuple", s_ends(("na", "ba")))

b = b"banana"
b_find = b.find
b_rfind = b.rfind
b_starts = b.startswith
b_ends = b.endswith

show("bytes_find", b_find(b"na"))
show("bytes_find_range", b_find(b"na", 2, 5))
show("bytes_rfind", b_rfind(b"na"))
show("bytes_rfind_range", b_rfind(b"na", 0, 4))
show("bytes_starts", b_starts(b"ba"))
show("bytes_starts_range", b_starts(b"na", 2))
show("bytes_starts_range2", b_starts(b"na", 2, 4))
show("bytes_starts_tuple", b_starts((b"ba", b"na")))
show("bytes_ends", b_ends(b"na"))
show("bytes_ends_range", b_ends(b"na", 0, 4))
show("bytes_ends_tuple", b_ends((b"na", b"ba")))

ba = bytearray(b"banana")
ba_find = ba.find
ba_rfind = ba.rfind
ba_starts = ba.startswith
ba_ends = ba.endswith

show("bytearray_find", ba_find(b"na"))
show("bytearray_find_range", ba_find(b"na", 2, 5))
show("bytearray_rfind", ba_rfind(b"na"))
show("bytearray_rfind_range", ba_rfind(b"na", 0, 4))
show("bytearray_starts", ba_starts(b"ba"))
show("bytearray_starts_range", ba_starts(b"na", 2))
show("bytearray_starts_range2", ba_starts(b"na", 2, 4))
show("bytearray_starts_tuple", ba_starts((b"ba", b"na")))
show("bytearray_ends", ba_ends(b"na"))
show("bytearray_ends_range", ba_ends(b"na", 0, 4))
show("bytearray_ends_tuple", ba_ends((b"na", b"ba")))

s_split = s.split
s_rsplit = s.rsplit
s_replace = s.replace
show("str_split", s_split("a"))
show("str_split_max", s_split("a", 1))
show("str_rsplit", s_rsplit("a"))
show("str_rsplit_max", s_rsplit("a", 1))
show("str_replace", s_replace("a", "o"))
show("str_replace_count", s_replace("a", "o", 1))

b_split = b.split
b_rsplit = b.rsplit
b_replace = b.replace
show("bytes_split", b_split(b"a"))
show("bytes_split_max", b_split(b"a", 1))
show("bytes_rsplit", b_rsplit(b"a"))
show("bytes_rsplit_max", b_rsplit(b"a", 1))
show("bytes_replace", b_replace(b"a", b"o"))
show("bytes_replace_count", b_replace(b"a", b"o", 1))

ba_split = ba.split
ba_rsplit = ba.rsplit
ba_replace = ba.replace
show("bytearray_split", ba_split(b"a"))
show("bytearray_split_max", ba_split(b"a", 1))
show("bytearray_rsplit", ba_rsplit(b"a"))
show("bytearray_rsplit_max", ba_rsplit(b"a", 1))
show("bytearray_replace", ba_replace(b"a", b"o"))
show("bytearray_replace_count", ba_replace(b"a", b"o", 1))

s_lines = "a\nb\r\nc"
s_splitlines = s_lines.splitlines
show("str_splitlines", s_splitlines())
show("str_splitlines_keep", s_splitlines(True))
show("str_partition", s.partition("na"))
show("str_rpartition", s.rpartition("na"))

b_lines = b"a\nb\r\nc"
b_splitlines = b_lines.splitlines
show("bytes_splitlines", b_splitlines())
show("bytes_splitlines_keep", b_splitlines(True))
show("bytes_partition", b.partition(b"na"))
show("bytes_rpartition", b.rpartition(b"na"))

ba_lines = bytearray(b"a\nb\r\nc")
ba_splitlines = ba_lines.splitlines
show("bytearray_splitlines", ba_splitlines())
show("bytearray_splitlines_keep", ba_splitlines(True))
show("bytearray_partition", ba.partition(b"na"))
show("bytearray_rpartition", ba.rpartition(b"na"))

joiner = "-".join
show("str_join", joiner(["a", "b", "c"]))

show("str_partition_miss", "hello".partition("zz"))
show("str_rpartition_miss", "hello".rpartition("zz"))
show("bytes_partition_miss", b"hello".partition(b"zz"))
show("bytes_rpartition_miss", b"hello".rpartition(b"zz"))
show("bytearray_partition_miss", bytearray(b"hello").partition(b"zz"))
show("bytearray_rpartition_miss", bytearray(b"hello").rpartition(b"zz"))

s_strip = "--hello--".strip
s_lstrip = "--hello--".lstrip
s_rstrip = "--hello--".rstrip
show("str_strip", s_strip("-"))
show("str_lstrip", s_lstrip("-"))
show("str_rstrip", s_rstrip("-"))

s_empty_split = "".splitlines
show("str_splitlines_empty", s_empty_split())
show("str_splitlines_empty_keep", s_empty_split(True))
s_trailing_split = "a\n".splitlines
show("str_splitlines_trailing", s_trailing_split())
show("str_splitlines_trailing_keep", s_trailing_split(True))

b_strip = b"--hello--".strip
b_lstrip = b"--hello--".lstrip
b_rstrip = b"--hello--".rstrip
show("bytes_strip", b_strip(b"-"))
show("bytes_lstrip", b_lstrip(b"-"))
show("bytes_rstrip", b_rstrip(b"-"))

ba_strip = bytearray(b"--hello--").strip
ba_lstrip = bytearray(b"--hello--").lstrip
ba_rstrip = bytearray(b"--hello--").rstrip
show("bytearray_strip", ba_strip(b"-"))
show("bytearray_lstrip", ba_lstrip(b"-"))
show("bytearray_rstrip", ba_rstrip(b"-"))

b_empty_split = b"".splitlines
show("bytes_splitlines_empty", b_empty_split())
show("bytes_splitlines_empty_keep", b_empty_split(True))
b_trailing_split = b"a\n".splitlines
show("bytes_splitlines_trailing", b_trailing_split())
show("bytes_splitlines_trailing_keep", b_trailing_split(True))

ba_empty_split = bytearray(b"").splitlines
show("bytearray_splitlines_empty", ba_empty_split())
show("bytearray_splitlines_empty_keep", ba_empty_split(True))
ba_trailing_split = bytearray(b"a\n").splitlines
show("bytearray_splitlines_trailing", ba_trailing_split())
show("bytearray_splitlines_trailing_keep", ba_trailing_split(True))


def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


show_err("bytes_strip_err_str", lambda: b"hi".strip("h"))
show_err("bytes_strip_err_int", lambda: b"hi".strip(1))
show("bytes_strip_mv", b"hi".strip(memoryview(b"h")))
show_err("bytearray_strip_err_str", lambda: bytearray(b"hi").strip("h"))
show_err("bytearray_strip_err_int", lambda: bytearray(b"hi").strip(1))
show("bytearray_strip_mv", bytearray(b"hi").strip(memoryview(b"h")))

show("str_splitlines_kw", s_lines.splitlines(keepends=True))
show("bytes_splitlines_kw", b_lines.splitlines(keepends=True))
show("bytearray_splitlines_kw", ba_lines.splitlines(keepends=True))

show_err("bytes_split_err_str", lambda: b"hi".split("h"))
show_err("bytes_split_err_int", lambda: b"hi".split(1))
show_err("bytes_rsplit_err_str", lambda: b"hi".rsplit("h"))
show_err("bytes_replace_err_str", lambda: b"hi".replace("h", b"x"))
show_err("bytes_replace_err_int", lambda: b"hi".replace(1, b"x"))
show_err("bytes_replace_err_to_str", lambda: b"hi".replace(b"h", "x"))

show_err("bytearray_split_err_str", lambda: bytearray(b"hi").split("h"))
show_err("bytearray_split_err_int", lambda: bytearray(b"hi").split(1))
show_err("bytearray_rsplit_err_str", lambda: bytearray(b"hi").rsplit("h"))
show_err("bytearray_replace_err_str", lambda: bytearray(b"hi").replace("h", b"x"))
show_err("bytearray_replace_err_int", lambda: bytearray(b"hi").replace(1, b"x"))
show_err("bytearray_replace_err_to_str", lambda: bytearray(b"hi").replace(b"h", "x"))

show_err("str_split_err_bytes", lambda: "hi".split(b"h"))
show_err("str_rsplit_err_bytes", lambda: "hi".rsplit(b"h"))
show_err("str_replace_err_bytes", lambda: "hi".replace(b"h", "x"))

show_err("str_starts_tuple_bad", lambda: "hi".startswith(("x", 1)))
show_err("str_ends_tuple_bad", lambda: "hi".endswith(("x", 1)))
show_err("bytes_starts_tuple_bad", lambda: b"hi".startswith((b"x", 1)))
show_err("bytes_ends_tuple_bad", lambda: b"hi".endswith((b"x", 1)))
show_err("bytearray_starts_tuple_bad", lambda: bytearray(b"hi").startswith((b"x", 1)))
show_err("bytearray_ends_tuple_bad", lambda: bytearray(b"hi").endswith((b"x", 1)))

show_err("str_starts_int", lambda: "hi".startswith(1))
show_err("str_ends_int", lambda: "hi".endswith(1))
show_err("bytes_starts_int", lambda: b"hi".startswith(1))
show_err("bytes_ends_int", lambda: b"hi".endswith(1))
show_err("bytearray_starts_int", lambda: bytearray(b"hi").startswith(1))
show_err("bytearray_ends_int", lambda: bytearray(b"hi").endswith(1))

show_err("str_partition_bytes", lambda: "hi".partition(b"h"))
show_err("bytes_partition_str", lambda: b"hi".partition("h"))
show_err("bytearray_partition_str", lambda: bytearray(b"hi").partition("h"))

show_err("bytes_find_tuple", lambda: b"hi".find((b"h", b"i")))
show_err("bytes_rfind_tuple", lambda: b"hi".rfind((b"h", b"i")))
show_err("bytearray_find_tuple", lambda: bytearray(b"hi").find((b"h", b"i")))
show_err("bytearray_rfind_tuple", lambda: bytearray(b"hi").rfind((b"h", b"i")))
show_err("bytes_count_tuple", lambda: b"hi".count((b"h", b"i")))
show_err("bytearray_count_tuple", lambda: bytearray(b"hi").count((b"h", b"i")))

show_err("bytes_replace_count_float", lambda: b"hi".replace(b"h", b"x", 1.2))
show_err("str_replace_count_float", lambda: "hi".replace("h", "x", 1.2))
show_err(
    "bytearray_replace_count_float", lambda: bytearray(b"hi").replace(b"h", b"x", 1.2)
)

show_err("bytes_splitlines_args", lambda: b"hi".splitlines(True, False))
show_err("str_splitlines_kw_bad", lambda: "hi".splitlines(bad=True))
show_err("bytes_splitlines_kw_bad", lambda: b"hi".splitlines(bad=True))
show_err("bytearray_splitlines_kw_bad", lambda: bytearray(b"hi").splitlines(bad=True))

show_err("str_count_err_int", lambda: "hi".count(1))

show("bytes_count_int", b"abracadabra".count(97))
show("bytes_count_int_slice", b"abracadabra".count(97, 1, 6))
show_err("bytes_count_err_oob", lambda: b"hi".count(256))
show_err("bytes_count_err_str", lambda: b"hi".count("h"))
show("bytearray_count_int", bytearray(b"abracadabra").count(97))
show("bytearray_count_int_slice", bytearray(b"abracadabra").count(97, 1, 6))
show_err("bytearray_count_err_oob", lambda: bytearray(b"hi").count(256))
show_err("bytearray_count_err_str", lambda: bytearray(b"hi").count("h"))
