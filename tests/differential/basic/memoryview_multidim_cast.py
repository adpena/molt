"""Purpose: differential coverage for memoryview multidim cast."""

mv = memoryview(b"abcd")
mv2 = mv.cast("B", (2, 2))
print(mv2.tolist())
