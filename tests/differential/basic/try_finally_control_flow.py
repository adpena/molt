"""Purpose: differential coverage for try/finally control-flow interactions."""


def return_override():
    try:
        return "try"
    finally:
        return "finally"


def raise_override():
    try:
        raise ValueError("inner")
    finally:
        raise KeyError("outer")


def loop_controls():
    out = []
    for i in range(3):
        try:
            if i == 1:
                continue
            if i == 2:
                break
        finally:
            out.append(f"finally{i}")
    return out


print("return", return_override())
try:
    raise_override()
except Exception as exc:
    print("raise", type(exc).__name__, str(exc))
print("loop", loop_controls())
