# Parity test: module system and imports
# All output via print() for diff comparison
# Note: We test only stdlib imports that don't require filesystem/network

print("=== import math ===")
import math
print(math.pi)
print(math.e)
print(math.sqrt(16))
print(math.floor(3.7))
print(math.ceil(3.2))
print(math.gcd(12, 8))
print(math.factorial(10))
print(math.log(math.e))
print(math.log2(8))
print(math.log10(1000))
print(math.isnan(float('nan')))
print(math.isinf(float('inf')))
print(math.isfinite(42.0))
print(math.copysign(1.0, -1.0))
print(math.fabs(-3.14))

print("=== from math import ===")
from math import sin, cos, tan, atan2
print(round(sin(0), 10))
print(round(cos(0), 10))
print(round(tan(0), 10))
print(round(atan2(1, 1), 10))

print("=== import sys basics ===")
import sys
print(type(sys.maxsize).__name__)
print(sys.maxsize > 0)
print(type(sys.version).__name__)
print(type(sys.platform).__name__)
print(type(sys.path).__name__)

print("=== import collections ===")
from collections import OrderedDict, defaultdict, Counter, deque

print("--- OrderedDict ---")
od = OrderedDict([("a", 1), ("b", 2), ("c", 3)])
print(list(od.keys()))
print(list(od.values()))

print("--- defaultdict ---")
dd = defaultdict(int)
dd["a"] += 1
dd["b"] += 2
dd["a"] += 3
print(sorted(dd.items()))

dd2 = defaultdict(list)
dd2["a"].append(1)
dd2["a"].append(2)
dd2["b"].append(3)
print(sorted((k, v) for k, v in dd2.items()))

print("--- Counter ---")
c = Counter("abracadabra")
print(sorted(c.items()))
print(c.most_common(3))
print(c["a"])
print(c["z"])

print("--- deque ---")
d = deque([1, 2, 3])
d.append(4)
d.appendleft(0)
print(list(d))
d.pop()
d.popleft()
print(list(d))
d.rotate(1)
print(list(d))

print("=== import itertools ===")
import itertools

print(list(itertools.chain([1, 2], [3, 4], [5])))
print(list(itertools.repeat("x", 3)))
print(list(itertools.islice(range(100), 5)))
print(list(itertools.islice(range(100), 2, 8, 2)))

print(list(itertools.accumulate([1, 2, 3, 4, 5])))
print(list(itertools.accumulate([1, 2, 3, 4], lambda x, y: x * y)))

for k, g in itertools.groupby("AAABBCCDDDA"):
    print(f"{k}: {list(g)}")

print(list(itertools.product("AB", "12")))
print(list(itertools.permutations("ABC", 2)))
print(list(itertools.combinations("ABCD", 2)))
print(list(itertools.combinations_with_replacement("AB", 3)))

print("=== import functools ===")
from functools import reduce, partial

print(reduce(lambda x, y: x + y, [1, 2, 3, 4, 5]))
print(reduce(lambda x, y: x * y, [1, 2, 3, 4, 5]))
print(reduce(lambda x, y: x + y, [], 0))

add5 = partial(lambda x, y: x + y, 5)
print(add5(10))
print(add5(20))

print("=== import operator ===")
import operator

print(operator.add(2, 3))
print(operator.sub(10, 4))
print(operator.mul(3, 7))
print(operator.truediv(10, 3))
print(operator.floordiv(10, 3))
print(operator.mod(10, 3))
print(operator.neg(-5))
print(operator.pos(-5))

print("=== import string ===")
import string
print(string.ascii_lowercase)
print(string.ascii_uppercase)
print(string.digits)
print(string.punctuation[:10])

print("=== import copy ===")
import copy

a = [1, [2, 3], [4, [5]]]
b = copy.copy(a)
c = copy.deepcopy(a)

a[1].append(99)
print(a)
print(b)
print(c)

print("=== import json ===")
import json

data = {"name": "Alice", "age": 30, "items": [1, 2, 3]}
s = json.dumps(data, sort_keys=True)
print(s)
parsed = json.loads(s)
print(sorted(parsed.items()))

print(json.dumps(None))
print(json.dumps(True))
print(json.dumps(42))
print(json.dumps("hello"))
print(json.dumps([1, 2, 3]))

print("=== __name__ ===")
print(__name__)

print("=== Module attributes ===")
print(hasattr(math, 'pi'))
print(hasattr(math, 'nonexistent'))
print(type(math).__name__)

print("=== import as ===")
import math as m
print(m.sqrt(25))

from math import floor as fl
print(fl(3.7))
