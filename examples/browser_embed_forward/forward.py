from array import array


def forward(raw: bytes):
    values = array("f")
    values.frombytes(raw)
    out = array("f")
    for value in values:
        out.append(value * 1.5 + 0.25)
    return out.tobytes()
