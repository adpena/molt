"""Extended differential coverage for negative indexing on all indexable types.

Covers: array.array, collections.deque, memoryview, and edge cases
(empty containers, single element, boundary indices, error messages).
"""
import array
from collections import deque


def show(label, value):
    print(label, value)


def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


# ── array.array ──────────────────────────────────────────────────────────────

arr = array.array("i", [10, 20, 30])
show("arr_-1", arr[-1])
show("arr_-3", arr[-3])
show_err("arr_-4", lambda: arr[-4])

arr_mut = array.array("i", [10, 20, 30])
arr_mut[-1] = 99
show("arr_set_-1", list(arr_mut))
show_err("arr_set_-4", lambda: arr_mut.__setitem__(-4, 1))

# array.array does not support __delitem__ via del on all Python versions;
# test pop with negative index instead.
arr_pop = array.array("i", [10, 20, 30])
show("arr_pop_-1", arr_pop.pop(-1))
show("arr_pop_after", list(arr_pop))
show("arr_pop_-2", arr_pop.pop(-2))
show("arr_pop_after2", list(arr_pop))
show_err("arr_pop_-5", lambda: arr_pop.pop(-5))

# empty array
arr_empty = array.array("i")
show_err("arr_empty_0", lambda: arr_empty[0])
show_err("arr_empty_-1", lambda: arr_empty[-1])
show_err("arr_empty_pop", lambda: arr_empty.pop())

# single element array
arr_single = array.array("i", [42])
show("arr_single_0", arr_single[0])
show("arr_single_-1", arr_single[-1])
show_err("arr_single_1", lambda: arr_single[1])
show_err("arr_single_-2", lambda: arr_single[-2])


# ── collections.deque ────────────────────────────────────────────────────────

dq = deque([100, 200, 300])
show("dq_-1", dq[-1])
show("dq_-3", dq[-3])
show_err("dq_-4", lambda: dq[-4])

dq_mut = deque([100, 200, 300])
dq_mut[-1] = 999
show("dq_set_-1", list(dq_mut))
show_err("dq_set_-4", lambda: dq_mut.__setitem__(-4, 1))

del dq_mut[-1]
show("dq_del_-1", list(dq_mut))
show_err("dq_del_-5", lambda: dq_mut.__delitem__(-5))

# empty deque
dq_empty = deque()
show_err("dq_empty_0", lambda: dq_empty[0])
show_err("dq_empty_-1", lambda: dq_empty[-1])

# single element deque
dq_single = deque([42])
show("dq_single_0", dq_single[0])
show("dq_single_-1", dq_single[-1])
show_err("dq_single_1", lambda: dq_single[1])
show_err("dq_single_-2", lambda: dq_single[-2])

# deque type errors — must match CPython format
show_err("dq_type_str", lambda: dq["a"])
show_err("dq_type_float", lambda: dq[1.5])
show_err("dq_type_none", lambda: dq[None])

# deque with __index__ protocol
class MyIdx:
    def __index__(self):
        return -1

show("dq_index_proto", dq[MyIdx()])

# deque with bool indexing (bools are ints)
show("dq_bool_true", dq[True])
show("dq_bool_false", dq[False])


# ── memoryview ───────────────────────────────────────────────────────────────

mv = memoryview(b"abcde")
show("mv_-1", mv[-1])
show("mv_-5", mv[-5])
show_err("mv_-6", lambda: mv[-6])

# writable memoryview
mv_owner = bytearray(b"abcde")
mv_w = memoryview(mv_owner)
mv_w[-1] = ord("z")
show("mv_w_set_-1", list(mv_owner))
mv_w[-5] = ord("A")
show("mv_w_set_-5", list(mv_owner))
show_err("mv_w_set_-6", lambda: mv_w.__setitem__(-6, 1))

# empty memoryview
mv_empty = memoryview(b"")
show_err("mv_empty_0", lambda: mv_empty[0])
show_err("mv_empty_-1", lambda: mv_empty[-1])

# single byte memoryview
mv_single = memoryview(b"x")
show("mv_single_0", mv_single[0])
show("mv_single_-1", mv_single[-1])
show_err("mv_single_1", lambda: mv_single[1])
show_err("mv_single_-2", lambda: mv_single[-2])

# memoryview slice with negative indices
mv_slice = memoryview(b"abcde")[-3:]
show("mv_slice_len", len(mv_slice))
show("mv_slice_0", mv_slice[0])
show("mv_slice_-1", mv_slice[-1])

# memoryview delete always fails
show_err("mv_del", lambda: mv_w.__delitem__(0))
show_err("mv_del_ro", lambda: mv.__delitem__(0))


# ── Boundary indices across types ────────────────────────────────────────────

# Verify that index == -len hits the first element (boundary)
for label, container in [
    ("list", [10, 20, 30]),
    ("tuple", (10, 20, 30)),
    ("str", "abc"),
    ("bytes", b"abc"),
    ("bytearray", bytearray(b"abc")),
    ("range", range(10, 40, 10)),
]:
    n = len(container)
    show(f"{label}_neg_len", container[-n])
    show_err(f"{label}_neg_len_minus1", lambda c=container, m=n: c[-(m + 1)])


# ── Negative index with slicing ──────────────────────────────────────────────

lst = [0, 1, 2, 3, 4]
show("slice_neg_start", lst[-3:])
show("slice_neg_stop", lst[:-2])
show("slice_neg_both", lst[-4:-1])
show("slice_neg_step", lst[-1:-6:-2])

s = "abcde"
show("str_slice_neg", s[-3:])
show("str_slice_neg_both", s[-4:-1])

b = b"abcde"
show("bytes_slice_neg", b[-3:])
show("bytes_slice_neg_both", b[-4:-1])

ba = bytearray(b"abcde")
show("ba_slice_neg", ba[-3:])
show("ba_slice_neg_both", ba[-4:-1])

# Slice assignment with negative indices
lst_assign = [0, 1, 2, 3, 4]
lst_assign[-3:] = [7, 8, 9]
show("slice_assign_neg", lst_assign)

lst_assign2 = [0, 1, 2, 3, 4]
lst_assign2[-4:-1] = [77]
show("slice_assign_neg_both", lst_assign2)

# Slice deletion with negative indices
lst_del = [0, 1, 2, 3, 4]
del lst_del[-3:]
show("slice_del_neg", lst_del)

lst_del2 = [0, 1, 2, 3, 4]
del lst_del2[-4:-1]
show("slice_del_neg_both", lst_del2)
