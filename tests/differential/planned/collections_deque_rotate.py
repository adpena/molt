"""Purpose: differential coverage for collections.deque rotate/appendleft."""

from collections import deque

d = deque([1, 2, 3])
d.rotate(1)
print(list(d))
d.appendleft(0)
print(list(d))
