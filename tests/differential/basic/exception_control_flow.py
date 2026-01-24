"""Purpose: differential coverage for exception control flow."""
# Ensures exceptions from calls stop subsequent statements in the same block.


def fail():
    raise ValueError("boom")


out = []
try:
    fail()
    out.append("after")
except ValueError:
    out.append("caught")

print(",".join(out))
