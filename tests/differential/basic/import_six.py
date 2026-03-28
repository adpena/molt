# MOLT_SKIP: six runtime object model crash (index out of bounds in runtime)
"""Purpose: differential coverage for six library import.

Tests that the six compatibility library can be imported and its
basic attributes are accessible.  six exercises complex module-level
patterns: class definitions with inheritance, lists of class instances,
module-level for loops with setattr, and conditional class definitions.
"""
import six

print(six.PY3)
print(six.text_type.__name__)
names = [attr for attr in dir(six.moves) if not attr.startswith('_')]
print(len(names) > 0)
