"""Purpose: differential coverage for list iter dunders."""

items = [1, 2, 3]
exhit = iter(items)
empit = iter(items)
for _ in exhit:
    next(empit)
items.append(9)
print("exhit", list(exhit))
print("empit", list(empit))
print("items", items)

reset = [1, 2, 3]
reset.__init__([4, 5])
print("init", reset)
reset.__init__()
print("init-empty", reset)
try:
    reset.__init__(None)
except TypeError:
    print("init-typeerror")
else:
    print("init-noerror")

print("add-callable", callable(items.__add__))
print("mul-callable", callable(items.__mul__))
print("rmul-callable", callable(items.__rmul__))
print("iadd-callable", callable(items.__iadd__))
print("imul-callable", callable(items.__imul__))

print("mul", items.__mul__(2))
print("rmul", items.__rmul__(2))

try:
    items.__add__((1,))
except TypeError:
    print("add-typeerror")
else:
    print("add-noerror")

try:
    items.__mul__("x")
except TypeError:
    print("mul-typeerror")
else:
    print("mul-noerror")

try:
    items.__imul__("x")
except TypeError:
    print("imul-typeerror")
else:
    print("imul-noerror")

try:
    items.__iadd__(None)
except TypeError:
    print("iadd-typeerror")
else:
    print("iadd-noerror")
