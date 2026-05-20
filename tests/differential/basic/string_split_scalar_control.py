def row(flag: bool) -> str:
    fields = "10|NA|42|tail".split("|")
    first = fields[0]
    if flag:
        middle = fields[1]
    else:
        middle = fields[2]
    return first + ":" + middle + ":" + fields[3]


print(row(True))
print(row(False))
