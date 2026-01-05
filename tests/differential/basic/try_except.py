def header(name: str) -> None:
    print(f"-- {name} --")


header("case1")
try:
    print("try")
    raise ValueError("boom")
    print("after")
except ValueError:
    print("except")
finally:
    print("finally")
print("case1 done")

header("case2")
try:
    print("try")
except Exception:
    print("except")
else:
    print("else")
finally:
    print("finally")
print("case2 done")

header("case3")
try:
    try:
        raise KeyError("kaboom")
    except KeyError:
        print("inner except")
        raise RuntimeError("from except")
        print("unreached")
    finally:
        print("inner finally")
except RuntimeError:
    print("outer except")
print("case3 done")

header("case4")
try:
    try:
        raise ValueError("first")
    finally:
        print("finally before raise")
        raise RuntimeError("second")
        print("unreached finally")
except RuntimeError:
    print("outer except")
print("case4 done")

header("case5")
try:
    try:
        print("try")
    except Exception:
        print("except")
    else:
        print("else")
        raise ValueError("else boom")
        print("unreached else")
    finally:
        print("finally")
except ValueError:
    print("outer except")
print("case5 done")

header("case6")
try:
    try:
        raise ValueError("re")
    except ValueError:
        print("except")
        raise
    finally:
        print("finally")
except ValueError:
    print("outer except")
print("case6 done")

header("case7")
try:
    raise
except RuntimeError:
    print("no active")
print("case7 done")

header("case8")
try:
    try:
        raise ValueError("v")
    except KeyError:
        print("wrong")
    finally:
        print("finally")
except ValueError:
    print("outer")
print("case8 done")

header("case9")
try:
    raise KeyError("k")
except ValueError:
    print("value")
except KeyError:
    print("key")
else:
    print("else")
finally:
    print("finally")
print("case9 done")

header("case10")


def return_in_finally() -> str:
    try:
        raise ValueError("boom")
    finally:
        return "ret"


print(return_in_finally())
print("case10 done")

header("case11")


def return_value_order() -> str:
    x = "start"
    try:
        return x
    finally:
        x = "final"
        print("finally", x)


print(return_value_order())
print("case11 done")

header("case12")


def return_none() -> None:
    try:
        pass
    finally:
        pass


print(return_none() is None)
print("case12 done")
