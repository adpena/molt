"""Purpose: differential coverage for sys.intern and sys.getsizeof."""

import sys

s1 = sys.intern("hello_molt_test")
s2 = sys.intern("hello_molt_test")
print("intern identity:", s1 is s2)
print("intern value:", s1)
sz = sys.getsizeof(42)
print("getsizeof int:", type(sz).__name__)
print("getsizeof > 0:", sz > 0)
sz2 = sys.getsizeof("hello")
print("getsizeof str:", type(sz2).__name__)
print("getsizeof str > 0:", sz2 > 0)
