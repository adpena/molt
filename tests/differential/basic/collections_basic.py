from molt.stdlib import collections

d = collections.deque([1, 2])
d.appendleft(0)
d.append(3)
print(list(d))
print(d.pop(), d.popleft(), list(d))
d.extend([4, 5])
d.extendleft([-1, -2])
print(list(d))

d2 = collections.deque(maxlen=2)
d2.append(1)
d2.append(2)
d2.append(3)
print(list(d2))

c = collections.Counter("aba")
print(c["a"], c["b"], c["z"])
c.update({"a": 2})
print(c["a"])
c.update(b=3)
print(c["b"])
print(c.most_common(1))

dd = collections.defaultdict(list)
dd["a"].append(1)
print(dd["a"])

try:
    dd2 = collections.defaultdict(None)
    dd2["missing"]
    print("noerror")
except KeyError:
    print("keyerror")
