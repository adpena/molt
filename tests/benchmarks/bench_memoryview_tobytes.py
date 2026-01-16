data = b"0123456789abcdef" * 200
view = memoryview(data)
view2 = view.cast("B", shape=[40, 80])
view3 = view.cast("B", shape=[20, 10, 16])
i = 0
total = 0
while i < 1000:
    total += view.tobytes()[0]
    total += view2.tobytes()[0]
    total += view3.tobytes()[0]
    i += 1

print(total)
