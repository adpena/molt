"""Purpose: differential coverage for exception target cleanup (except ... as ...)."""

try:
    raise ValueError("boom")
except Exception as err:
    print("inside", type(err).__name__, str(err))

try:
    err
except Exception as exc:
    print("after", type(exc).__name__)
