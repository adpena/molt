"""Purpose: differential coverage for walrus binding before filter exceptions."""


def boom():
    raise RuntimeError("boom")


try:
    [
        (x := i)
        for i in range(3)
        if (x := i) == 1 and boom()
    ]
except Exception as exc:
    print("err", type(exc).__name__, x)
