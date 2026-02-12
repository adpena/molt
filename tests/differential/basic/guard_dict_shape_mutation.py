"""Purpose: differential coverage for guard_dict_shape under dict mutations."""

d = {"a": 1}
for key in ("a", "b", "a"):
    d[key] = d.get(key, 0) + 1
print(tuple(sorted(d.items())))

d["z"] = 100
for key in ("a", "c", "a", "z"):
    d[key] = d.get(key, 0) + 1
print(tuple(sorted(d.items())))
