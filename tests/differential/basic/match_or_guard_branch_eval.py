"""Purpose: differential coverage for OR-pattern guard eval only on match."""

def guard(label, events):
    events.append(label)
    return True


events = []
value = (3, 4)
match value:
    case (1, 2) | (3, 4) if guard("hit", events):
        print("match", "yes")
    case _:
        print("match", "no")
print("events", events)

events = []
value = (9, 9)
match value:
    case (1, 2) | (3, 4) if guard("miss", events):
        print("miss", "bad")
    case _:
        print("miss", "ok")
print("events2", events)
