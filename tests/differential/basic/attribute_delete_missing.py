"""Purpose: differential coverage for deleting missing attributes."""

class Demo:
    pass


if __name__ == "__main__":
    demo = Demo()
    try:
        del demo.missing
    except AttributeError as exc:
        print("error", exc.name, exc.obj is demo)


# Which failures record AttributeError.name and .obj depends on the version:
# a get always; a slots-only set or delete and a class delete from 3.13; a
# dict-backed instance's delete from 3.15.
class Slotted:
    __slots__ = ("a",)


demo, slotted = Demo(), Slotted()
for label, action, target in (
    ("del-dict", lambda: delattr(demo, "zz"), demo),
    ("del-slots-missing", lambda: delattr(slotted, "zz"), slotted),
    ("del-class", lambda: delattr(Demo, "zz"), Demo),
    ("set-slots-missing", lambda: setattr(slotted, "zz", 1), slotted),
    ("get-dict", lambda: getattr(demo, "zz"), demo),
    ("get-class", lambda: getattr(Demo, "zz"), Demo),
):
    try:
        action()
    except AttributeError as exc:
        print(label, str(exc), exc.name, exc.obj is target)
