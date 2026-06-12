"""Purpose: differential coverage for KeyError message parity."""


# 1. Missing string key
try:
    d = {"a": 1}
    d["b"]
except KeyError as e:
    print(f"KeyError: {e}")

# 2. Missing int key
try:
    d = {1: "one"}
    d[2]
except KeyError as e:
    print(f"KeyError: {e}")

# 3. Missing tuple key
try:
    d = {}
    d[(1, 2)]
except KeyError as e:
    print(f"KeyError: {e}")

# 4. Empty dict access
try:
    {}["anything"]
except KeyError as e:
    print(f"KeyError: {e}")

# 5. Dict pop with missing key (no default)
try:
    d = {"a": 1}
    d.pop("b")
except KeyError as e:
    print(f"KeyError: {e}")

# 6. Set remove missing element
try:
    s = {1, 2, 3}
    s.remove(99)
except KeyError as e:
    print(f"KeyError: {e}")

# 7. None as key
try:
    d = {}
    d[None]
except KeyError as e:
    print(f"KeyError: {e}")

# 8. Boolean key
try:
    d = {}
    d[True]
except KeyError as e:
    print(f"KeyError: {e}")

# 9. Float key
try:
    d = {}
    d[3.14]
except KeyError as e:
    print(f"KeyError: {e}")

# 10. Del on missing key
try:
    d = {"a": 1}
    del d["b"]
except KeyError as e:
    print(f"KeyError: {e}")
