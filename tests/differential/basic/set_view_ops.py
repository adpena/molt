view_src = {1: 10, 2: 20}

print(f"keys_or_set:{sorted(view_src.keys() | {0})}")
print(f"keys_and_set:{sorted(view_src.keys() & {2, 3})}")
print(f"keys_sub_set:{sorted(view_src.keys() - {1})}")
print(f"keys_xor_set:{sorted(view_src.keys() ^ {2, 3})}")
print(f"set_or_keys:{sorted({0} | view_src.keys())}")
print(f"set_and_keys:{sorted({0, 2} & view_src.keys())}")

items = view_src.items()
print(f"items_or_set:{sorted(items | {(3, 30)})}")
print(f"items_and_set:{sorted(items & {(1, 10), (3, 30)})}")
print(f"items_sub_set:{sorted(items - {(1, 10)})}")
print(f"items_xor_set:{sorted(items ^ {(1, 10), (3, 30)})}")
print(f"set_or_items:{sorted({(3, 30)} | items)}")

print(f"keys_or_type:{type(view_src.keys() | {0}).__name__}")
print(f"items_or_type:{type(items | {(3, 30)}).__name__}")
print(f"fs_or_keys_type:{type(frozenset({0}) | view_src.keys()).__name__}")

try:
    view_src.values() | {1}
except TypeError as exc:
    print(f"values_or_err:{exc}")
