"""Purpose: differential coverage for memoryview format codes."""

import array


arr = array.array("h", [1, 2, 3])
mv = memoryview(arr)
print(mv.format, mv.tolist())

mvb = mv.cast("B")
print(mvb.format, mvb.tolist())
