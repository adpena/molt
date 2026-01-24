"""Purpose: differential coverage for try/except/else return interplay."""


def try_else_return(flag):
    try:
        if flag:
            raise ValueError("boom")
        return "try"
    except Exception:
        return "except"
    else:
        return "else"


def try_else_finally(flag):
    try:
        if flag:
            raise KeyError("boom")
        return "try"
    except Exception:
        return "except"
    else:
        return "else"
    finally:
        return "finally"


print("try_else_false", try_else_return(False))
print("try_else_true", try_else_return(True))
print("try_else_finally_false", try_else_finally(False))
print("try_else_finally_true", try_else_finally(True))
