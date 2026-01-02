total = 0
i = 0
while i < 100:
    j = 0
    while j < 100:
        k = 0
        while k < 100:
            inner = 0
            while inner < 100:
                total = total + 1
                inner = inner + 1
            k = k + 1
        j = j + 1
    i = i + 1
print(total)
