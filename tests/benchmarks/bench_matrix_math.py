def mat_mul():
    i = 0
    res = 0
    while i < 1000000:
        c00 = 1 * 5 + 2 * 7
        c01 = 1 * 6 + 2 * 8
        c10 = 3 * 5 + 4 * 7
        c11 = 3 * 6 + 4 * 8
        res = res + c00 + c01 + c10 + c11
        i = i + 1
    return res

print(mat_mul())
