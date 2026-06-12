# Council #59 regression matrix, case 5/10: resurrect_with_field_refs.
#
# Resurrection with INNER references (council §D "inner-ref cascade ordering").
# The resurrecting instance owns references to other live objects via instance
# fields (a child object and a list). On the first death the object resurrects
# WITHOUT clearing/destroying its inner refs — they must stay valid and reachable
# through the resurrected object. On the final death the inner refs are released
# in the correct cascade order (the parent's payload is cleared, then the
# children's refcounts drop). A premature inner-ref clear at the zero-transition
# (before the resurrection check) would leave the resurrected object pointing at
# freed children -> UAF; this verifies that does NOT happen.
import gc

box = []


class Child:
    def __init__(self, tag):
        self.tag = tag


class Parent:
    def __init__(self):
        self.child = Child("c")
        self.items = [1, 2, 3]

    def __del__(self):
        box.append(self)


def run():
    p = Parent()
    del p  # __del__ -> resurrect; inner refs (child, items) must remain valid
    gc.collect()
    print("box_len", len(box))
    # Through the resurrected object, the inner refs are intact and usable.
    obj = box[0]
    print("child_tag", obj.child.tag)
    print("items", obj.items)
    obj.items.append(4)
    print("items_after", obj.items)
    # Final drop: parent + inner refs released in order, leak-clean.
    box.clear()
    gc.collect()
    print("after_final box_len", len(box))


run()
print("done")
