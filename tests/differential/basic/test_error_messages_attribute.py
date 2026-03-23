"""Purpose: differential coverage for AttributeError message parity."""


# 1. Attribute on int
try:
    x = 42
    x.foo
except AttributeError as e:
    print(f"AttributeError: {e}")

# 2. Attribute on string
try:
    "hello".nonexistent
except AttributeError as e:
    print(f"AttributeError: {e}")

# 3. Attribute on list
try:
    [1, 2].nonexistent
except AttributeError as e:
    print(f"AttributeError: {e}")

# 4. Attribute on dict
try:
    {}.nonexistent
except AttributeError as e:
    print(f"AttributeError: {e}")

# 5. Attribute on None
try:
    None.something
except AttributeError as e:
    print(f"AttributeError: {e}")

# 6. Attribute on bool
try:
    True.missing
except AttributeError as e:
    print(f"AttributeError: {e}")

# 7. Attribute on tuple
try:
    (1, 2).missing
except AttributeError as e:
    print(f"AttributeError: {e}")

# 8. Setting attribute on built-in type
try:
    x = 42
    x.foo = "bar"
except AttributeError as e:
    print(f"AttributeError: {e}")

# 9. Custom class missing attribute
class MyClass:
    pass

try:
    obj = MyClass()
    obj.nonexistent
except AttributeError as e:
    print(f"AttributeError: {e}")

# 10. Module attribute (via object)
try:
    object.nonexistent_method
except AttributeError as e:
    print(f"AttributeError: {e}")

# 11. Deleting attribute that does not exist
class Simple:
    pass

try:
    s = Simple()
    del s.nope
except AttributeError as e:
    print(f"AttributeError: {e}")
