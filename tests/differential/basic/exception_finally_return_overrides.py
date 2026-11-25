"""Purpose: differential coverage for finally return overriding exceptions."""


def run():
    try:
        raise ValueError("boom")
    finally:
        return "done"


print("result", run())


marker = ValueError("original")


def leaf(fail):
    if fail:
        raise marker
    return 7


def bridge(fail):
    try:
        return leaf(fail)
    finally:
        print("cleanup", fail)


print("success", bridge(False))
try:
    bridge(True)
except ValueError as caught:
    print("original exception", caught is marker)
