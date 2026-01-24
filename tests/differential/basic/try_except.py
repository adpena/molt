"""Purpose: differential coverage for try except."""


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

header("case13")
try:
    try:
        print("try")
    except Exception:
        print("except")
    else:
        print("else")
        raise RuntimeError("else boom")
    finally:
        print("finally")
except RuntimeError:
    print("outer except")
print("case13 done")

header("case14")
try:
    try:
        raise ValueError("inner")
    except ValueError:
        print("except")
        raise RuntimeError("raised")
    else:
        print("else")
    finally:
        print("finally")
except RuntimeError:
    print("outer except")
print("case14 done")

header("case15")
try:
    try:
        print("try")
    finally:
        print("finally")
        raise KeyError("k")
except KeyError:
    print("outer except")
print("case15 done")

header("case16")
try:
    try:
        raise ValueError("inner")
    except ValueError:
        raise RuntimeError("outer")
except RuntimeError as exc:
    print(exc.__context__)
    print(exc.__cause__ is None)
    print(exc.__suppress_context__)
print("case16 done")

header("case17")
try:
    try:
        raise ValueError("inner")
    except ValueError as err:
        raise RuntimeError("outer") from err
except RuntimeError as exc:
    print(exc.__context__)
    print(exc.__cause__)
    print(exc.__suppress_context__)
print("case17 done")

header("case18")
try:
    try:
        raise ValueError("inner")
    except ValueError:
        raise RuntimeError("outer") from None
except RuntimeError as exc:
    print(exc.__context__)
    print(exc.__cause__ is None)
    print(exc.__suppress_context__)
print("case18 done")

header("case19")
try:
    raise ValueError("inner")
except ValueError as err:
    try:
        raise RuntimeError("ctx")
    except RuntimeError as ctx:
        err.__context__ = ctx
    err.__cause__ = None
    err.__suppress_context__ = True
    print(err.__context__)
    print(err.__cause__ is None)
    print(err.__suppress_context__)
print("case19 done")

header("case20")
try:
    raise ValueError("inner")
except ValueError as err:
    try:
        err.__cause__ = "bad"
    except TypeError:
        print("typeerror")
print("case20 done")
