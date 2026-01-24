"""Purpose: differential coverage for finally return overriding exceptions."""


def run():
    try:
        raise ValueError("boom")
    finally:
        return "done"


print("result", run())
