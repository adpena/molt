"""Purpose: differential coverage for repr recursion."""

lst = []
lst.append(lst)
print(lst)

d = {"self": None}
d["self"] = d
print(d)
