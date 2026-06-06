"""Purpose: differential coverage for NameError message parity."""


# 1. Undefined variable
try:
    print(undefined_var)
except NameError as e:
    print(f"NameError: {e}")

# 2. Undefined in expression
try:
    x = missing_name + 1
except NameError as e:
    print(f"NameError: {e}")

# 3. Undefined function call
try:
    nonexistent_func()
except NameError as e:
    print(f"NameError: {e}")

# 4. UnboundLocalError (subclass of NameError)
def closure_bug():
    x = 10
    def inner():
        print(x)
        x = 20
    inner()

try:
    closure_bug()
except UnboundLocalError as e:
    print(f"UnboundLocalError: {e}")

# 5. UnboundLocalError — reference before assignment
def ref_before_assign():
    print(y)
    y = 5

try:
    ref_before_assign()
except UnboundLocalError as e:
    print(f"UnboundLocalError: {e}")

# 6. Name error in list comprehension
try:
    result = [whoops for _ in range(3)]
except NameError as e:
    print(f"NameError: {e}")

# 7. Name error in conditional
try:
    if nonexistent_flag:
        pass
except NameError as e:
    print(f"NameError: {e}")

# 8. Del then access
try:
    temp = 42
    del temp
    print(temp)
except NameError as e:
    print(f"NameError: {e}")

# 9. Global declaration but never assigned
def use_global():
    global never_assigned_global
    print(never_assigned_global)

try:
    use_global()
except NameError as e:
    print(f"NameError: {e}")

# 10. Misspelled builtin
try:
    x = Tru
except NameError as e:
    print(f"NameError: {e}")
