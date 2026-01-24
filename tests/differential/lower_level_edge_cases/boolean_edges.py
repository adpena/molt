"""Purpose: differential coverage for boolean edges."""


def section(name):
    print(f"--- {name} ---")


def truthy(val, label):
    print(f"Checking {label}")
    return val


section("Short-circuiting 'and'")
# If first is False, second is not evaluated
if truthy(False, "A") and truthy(True, "B"):
    print("Both True")
else:
    print("Short-circuited AND")

if truthy(True, "A") and truthy(True, "B"):
    print("Full AND")

section("Short-circuiting 'or'")
# If first is True, second is not evaluated
if truthy(True, "A") or truthy(False, "B"):
    print("Short-circuited OR")

if truthy(False, "A") or truthy(True, "B"):
    print("Full OR")

section("Truthiness of empty/zero")
values = [0, 0.0, -0.0, None, False, "", b"", [], (), {}, set(), [1], (1,), {1}, {1: 1}]

for v in values:
    if v:
        print(f"True: {repr(v)}")
    else:
        print(f"False: {repr(v)}")

section("Not operator")
print(not True)
print(not False)
print(not [])
print(not [1])
