"""Purpose: differential coverage for set method attrs."""


def show(label, value):
    print(label, value)


def show_set(label, value):
    print(label, sorted(value))


def show_err(label, func):
    try:
        func()
    except Exception as exc:
        print(label, type(exc).__name__, exc)


s = {1, 2}
show_set("set_union_bound", s.union({2, 3}))
show_set("set_union_multi_bound", s.union({3}, [4, 1]))
show_set("set_intersection_bound", s.intersection({2, 4}))
show_set("set_intersection_multi_bound", s.intersection({1, 2, 3}, [2, 5]))
show_set("set_difference_bound", s.difference([2, 5]))
show_set("set_difference_multi_bound", s.difference({2}, [1]))
show_set("set_symdiff_bound", s.symmetric_difference({2, 3}))
show("set_isdisjoint", s.isdisjoint({3, 4}))
show("set_issubset", s.issubset({1, 2, 3}))
show("set_issuperset", s.issuperset({1}))
show("set_copy_is", s.copy() is s)

s_update = {1, 2}
show("set_update_ret", s_update.update([2, 3], {4}))
show_set("set_update_after", s_update)
show("set_update_empty_ret", s_update.update())
show_set("set_update_empty_after", s_update)

s_inter = {1, 2, 3}
show("set_intersection_update_ret", s_inter.intersection_update({2, 3}, [3, 4]))
show_set("set_intersection_update_after", s_inter)

s_diff = {1, 2, 3}
show("set_difference_update_ret", s_diff.difference_update({2}, [3]))
show_set("set_difference_update_after", s_diff)

s_sym = {1, 2, 3}
show("set_symdiff_update_ret", s_sym.symmetric_difference_update({2, 4}))
show_set("set_symdiff_update_after", s_sym)

s_clear = {1, 2}
show("set_clear_ret", s_clear.clear())
show("set_clear_len", len(s_clear))

show_err("set_union_kw", lambda: s.union(bad=1))
show_err("set_symdiff_0", lambda: s.symmetric_difference())
show_err("set_symdiff_2", lambda: s.symmetric_difference(1, 2))
show_err("set_copy_1", lambda: s.copy(1))
show_err("set_clear_1", lambda: s.clear(1))
show_err("set_union_wrong_self_list", lambda: set.union([1], {2}))
show_err("set_union_wrong_self_frozen", lambda: set.union(frozenset({1}), {2}))

fs = frozenset([1, 2])
show_set("frozenset_union_bound", fs.union({2, 3}))
show_set("frozenset_union_multi_bound", fs.union({3}, [4, 1]))
show_set("frozenset_intersection_bound", fs.intersection({2, 4}))
show_set("frozenset_intersection_multi_bound", fs.intersection({1, 2, 3}, [2, 5]))
show_set("frozenset_difference_bound", fs.difference([2, 5]))
show_set("frozenset_difference_multi_bound", fs.difference({2}, [1]))
show_set("frozenset_symdiff_bound", fs.symmetric_difference({2, 3}))
show("frozenset_isdisjoint", fs.isdisjoint({3, 4}))
show("frozenset_issubset", fs.issubset({1, 2, 3}))
show("frozenset_issuperset", fs.issuperset({1}))
show("frozenset_copy_is", fs.copy() is fs)
show_err("frozenset_union_kw", lambda: fs.union(bad=1))
show_err("frozenset_symdiff_0", lambda: fs.symmetric_difference())
show_err("frozenset_union_wrong_self_set", lambda: frozenset.union({1}, {2}))
