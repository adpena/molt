"""Purpose: differential coverage for for else."""


def run():
    out = []
    for i in range(3):
        out.append(i)
    else:
        out.append("done")

    out2 = []
    for i in range(3):
        if i == 1:
            break
        out2.append(i)
    else:
        out2.append("done")

    out3 = []
    for i in []:
        out3.append(i)
    else:
        out3.append("empty")

    out4 = []
    for i in range(3):
        if i == 1:
            continue
        out4.append(i)
    else:
        out4.append("cont")

    out5 = []
    for key in {"a": 1, "b": 2}:
        out5.append(key)
    else:
        out5.append("dict_done")

    out6 = []
    i = 0
    while i < 3:
        out6.append(i)
        i += 1
    else:
        out6.append("done")

    out7 = []
    i = 0
    while i < 3:
        if i == 1:
            break
        out7.append(i)
        i += 1
    else:
        out7.append("done")

    out8 = []
    i = 0
    while i < 0:
        out8.append(i)
        i += 1
    else:
        out8.append("empty")

    nested = []
    for i in range(2):
        for j in range(2):
            if j == 1:
                break
            nested.append((i, j))
        else:
            nested.append(("inner", "done"))
    else:
        nested.append(("outer", "done"))

    def early():
        for i in range(3):
            if i == 1:
                return "stop"
        else:
            return "done"

    return out, out2, out3, out4, out5, out6, out7, out8, nested, early()


print(run())
