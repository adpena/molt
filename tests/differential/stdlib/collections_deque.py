"""Purpose: differential coverage for collections deque."""

import collections

d = collections.deque([1, 2, 3])
d.rotate(1)
print(list(d))
d.rotate(-2)
print(list(d))
d.rotate(-1000)
print(list(d))
d.appendleft(0)
d.append(4)
print(d[1], d[-1])
d[1] = 10
print(list(d))
print(d.count(1))
print(d.index(3))
try:
    print(d.index(3, 2, 2))
except ValueError:
    print("index-miss")
try:
    print(d.index(42))
except ValueError:
    print("index-miss2")
d.insert(2, 99)
print(list(d))
d.remove(99)
print(list(d))
d.reverse()
print(list(d))
print(d.maxlen)

bounded = collections.deque([1, 2], maxlen=2)
try:
    bounded.insert(0, 3)
    print("noerror")
except IndexError:
    print("full")
try:
    bounded.insert(-1, 4)
    print("noerror2")
except IndexError:
    print("full2")

copied = d.copy()
copied.append(7)
print(list(d), list(copied))

c1 = collections.Counter("abbb")
c2 = collections.Counter("bcc")
print(sorted(c1.items()))
print(sorted((c1 + c2).items()))
print(sorted((c1 - c2).items()))
print(sorted((c1 | c2).items()))
print(sorted((c1 & c2).items()))
print(sorted(list(c1.elements())))
print(isinstance(c1, dict))

dd = collections.defaultdict(list)
dd["a"].append(1)
print(dd["a"])
print(isinstance(dd, dict))

try:
    dd2 = collections.defaultdict(None)
    dd2["missing"]
    print("noerror")
except KeyError:
    print("keyerror")
