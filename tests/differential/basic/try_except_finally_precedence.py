"""Purpose: differential coverage for try/except/else/finally precedence."""


def try_else_finally():
    try:
        out = "try"
    except Exception:
        out = "except"
    else:
        out = "else"
    finally:
        out = "finally"
    return out


def try_except_return():
    try:
        raise ValueError("boom")
    except Exception:
        return "except"
    finally:
        return "finally"


def try_except_break():
    out = []
    for i in range(2):
        try:
            if i == 0:
                raise KeyError("x")
        except Exception:
            break
        finally:
            out.append(f"finally{i}")
    return out


print("try_else", try_else_finally())
print("try_except_return", try_except_return())
print("try_except_break", try_except_break())

try:
    raise RuntimeError("outer")
except RuntimeError:
    try:
        raise ValueError("inner")
    finally:
        pass
