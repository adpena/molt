"""Mutually exclusive inner loops preserve CPython branch semantics."""


def store_nested_branch(target, mapping):
    if hasattr(mapping, "items"):
        for key, item in mapping.items():
            target[key] = item
    else:
        for key, item in mapping:
            target[key] = item


items_target = {}
store_nested_branch(items_target, {"left": 1, "right": 2})
print("items-arm", items_target)

iter_target = {}
store_nested_branch(iter_target, [("left", 1), ("right", 2)])
print("iter-arm", iter_target)
