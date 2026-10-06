"""Sequence owner release ordering and publication under finalizer reentry."""
events = []
bag = None
reenter = False
class Item:
    def __init__(self, name):
        self.name = name
    def __del__(self):
        events.append((self.name, len(bag) if bag is not None else -1))
        if reenter:
            bag.append(self.name)

def values():
    return [Item("a"), Item("b"), Item("c"), Item("d"), Item("e")]

def run(case):
    global bag, reenter
    events.clear()
    bag = values()
    reenter = True
    if case == "clear":
        bag.clear()
    elif case == "contiguous_delete":
        del bag[1:4]
    elif case == "extended_delete":
        del bag[::2]
    elif case == "negative_delete":
        del bag[::-2]
    elif case == "contiguous_assign":
        bag[1:4] = [10]
    elif case == "extended_assign":
        bag[::2] = [10, 20, 30]
    elif case == "negative_assign":
        bag[::-2] = [10, 20, 30]
    elif case == "repeat_zero":
        bag *= 0
    print(case, events)
    reenter = False
    bag = None

for case in ("clear", "contiguous_delete", "extended_delete", "negative_delete", "contiguous_assign", "extended_assign", "negative_assign", "repeat_zero"):
    run(case)
events.clear()
pair = (Item("first"), Item("second"), Item("third"))
del pair
print("tuple_drop", events)
events.clear()
items = values()
del items
print("list_drop", events)
