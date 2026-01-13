from molt.stdlib import collections

d = collections.deque(maxlen=2)
d.append(1)
d.append(2)
d.append(3)
print(list(d))

dd = collections.defaultdict(default_factory=list)
dd["a"].append(1)
print(dd["a"])

dd2 = collections.defaultdict(default_factory=None, a=1, b=2)
print(list(dd2.items()))

c = collections.Counter("aba", b=2)
print(c["a"], c["b"])
