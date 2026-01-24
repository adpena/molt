"""Purpose: differential coverage for exception chaining in finally/else."""


def run(flag):
    try:
        if flag:
            raise KeyError("try")
    except Exception as exc:
        try:
            raise ValueError("except") from exc
        finally:
            pass
    else:
        try:
            raise RuntimeError("else")
        finally:
            raise OSError("finally")


try:
    run(True)
except Exception as exc:
    print("except", type(exc).__name__, type(exc.__cause__).__name__, type(exc.__context__).__name__)

try:
    run(False)
except Exception as exc:
    print("else", type(exc).__name__, type(exc.__context__).__name__)
