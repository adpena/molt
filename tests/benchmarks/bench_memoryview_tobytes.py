data = b"0123456789abcdef" * 200
view = memoryview(data)
i = 0
total = 0
while i < 1000:
    total += view.tobytes()[0]
    i += 1

print(total)
