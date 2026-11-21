"""A cell written by reflection keeps super() dynamic (design 78, 5.10 step 4).

Writing ``cell_contents`` of a method's ``__class__`` cell changes the class
its zero-argument super() starts after, so the program's graph names a cell
writer and no super() call is decided at compile time.
"""


class Base:
    def name(self):
        return "Base"


class Mid(Base):
    def name(self):
        return "Mid"


class Leaf(Mid):
    def name(self):
        return "Leaf>" + super().name()


def main():
    leaf = Leaf()
    print("before", leaf.name())
    # `__class__` is the method's only free variable. Reading `__code__` to
    # find it would make the graph's code-rebinding premise refuse instead.
    Leaf.name.__closure__[0].cell_contents = Mid
    print("after", leaf.name())


main()
