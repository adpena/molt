# Differential test: boolean operator semantics (or / and)
# Verifies that short-circuit evaluation returns the correct operand.

# --- or operator: returns first truthy operand, or last operand if all falsy ---

# Falsy left operand -> returns right
print(None or "hello")       # hello
print(0 or 42)               # 42
print("" or "world")         # world
print(False or True)          # True

# Truthy left operand -> returns left (short-circuit)
print("truthy" or "other")   # truthy
print(1 or 99)               # 1
print(True or False)          # True

# Chained or
print(None or 0 or "found")  # found
print(None or "" or 0)       # 0
print(1 or 2 or 3)           # 1
print(0 or 0 or 0)           # 0

# --- and operator: returns first falsy operand, or last operand if all truthy ---

# Falsy left operand -> returns left (short-circuit)
print(None and "hello")      # None
print(0 and 42)              # 0
print("" and "world")        # (empty string)
print(False and True)         # False

# Truthy left operand -> returns right
print("truthy" and "other")  # other
print(1 and 42)              # 42
print(True and False)         # False

# Chained and
print(1 and 2 and 3)         # 3
print(1 and 0 and 3)         # 0
print(None and 0 and 3)      # None

# --- Mixed or/and with variables ---
a = None
b = "hello"
c = a or b
print(c)                      # hello

d = "yes"
e = "no"
f = d and e
print(f)                      # no

# Nested: (a or b) and (c or d)
x = (None or 1) and (0 or 2)
print(x)                      # 2

y = (None or 0) and (1 or 2)
print(y)                      # 0

# Type preservation
print(type(0 or 42).__name__)          # int
print(type(None or "x").__name__)      # str
print(type("" and "x").__name__)       # str
print(type(True and 42).__name__)      # int
