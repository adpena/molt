"""Purpose: method calls fill trailing parameters from live positional defaults (HF-216).

A loaded method call that supplies fewer positionals than the method takes
gets the missing trailing values from the function's current __defaults__,
as CPython binds them: a reassigned or shortened __defaults__ applies at the
next call, too few arguments raise CPython's TypeError, mutable defaults stay
shared, and keyword-only or *args parameters still bind correctly.
"""


class C:
    def m(self, a, b=2, c=3):
        return (a, b, c)


c = C()
print(c.m(1), c.m(1, 20), c.m(1, 20, 30))
total = 0
for i in range(200):
    total += sum(c.m(i))
print(total)

C.m.__defaults__ = (200, 300)
print(c.m(1))
C.m.__defaults__ = (300,)
print(c.m(1, 2))
try:
    c.m(1)
except TypeError as error:
    print("TypeError", error)
C.m.__defaults__ = None
try:
    c.m(1, 2)
except TypeError as error:
    print("TypeError", error)
print(c.m(1, 2, 3))


class Shared:
    def m(self, acc=[]):
        acc.append(len(acc))
        return acc


s = Shared()
first = s.m()
print(first, s.m() is first, len(first))


class KeywordOnly:
    def m(self, a, b=2, *, k=5):
        return a + b + k


class Star:
    def m(self, a=1, *rest):
        return (a, rest)


print(KeywordOnly().m(1), KeywordOnly().m(1, 10, k=100))
print(Star().m(), Star().m(7), Star().m(1, 2, 3))


class Wide:
    def m(self, a, b=1, c=2, d=3, e=4, f=5, g=6, h=7, i=8, j=9, k=10, l=11,
          m=12, n=13, o=14, p=15, q=16):
        return a + b + c + d + e + f + g + h + i + j + k + l + m + n + o + p + q


print(Wide().m(0), Wide().m(0, 0, 0))
