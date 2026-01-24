"""Purpose: differential coverage for OR-pattern guard evaluation order."""

events = []


def guard(label):
    events.append(label)
    return False


value = (1, 2)
match value:
    case (1, 2) | (3, 4) if guard("first"):
        print("hit")
    case _:
        print("miss")

print("events", events)
