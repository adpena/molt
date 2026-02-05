def gen(flag):
    if flag:
        yield 1
    return


print("gen_true", list(gen(True)))
print("gen_false", list(gen(False)))


def gen2(flags):
    if flags & 2:
        yield "hit"
    return


print("gen2", list(gen2(2)))
print("gen2_zero", list(gen2(0)))
