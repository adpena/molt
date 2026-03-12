"""Purpose: differential coverage for PEP 695 generic class syntax (class Foo[T]:)."""


# Basic generic class
class Stack[T]:
    def __init__(self):
        self.items: list[T] = []

    def push(self, item: T) -> None:
        self.items.append(item)

    def pop(self) -> T:
        return self.items.pop()

    def peek(self) -> T:
        return self.items[-1]

    def size(self) -> int:
        return len(self.items)


# Generic class with multiple type params
class Pair[K, V]:
    def __init__(self, key: K, value: V):
        self.key = key
        self.value = value

    def swap(self) -> "Pair[V, K]":
        return Pair(self.value, self.key)


# Generic class with bound
class Container[T]:
    def __init__(self, value: T):
        self.value = value

    def get(self) -> T:
        return self.value

    def replace(self, new: T) -> "Container[T]":
        return Container(new)


# Generic subclass
class NumberStack[T](Stack[T]):
    def sum_all(self) -> T:
        total = 0
        for item in self.items:
            total += item
        return total


if __name__ == "__main__":
    # Basic Stack usage
    s = Stack()
    s.push(1)
    s.push(2)
    s.push(3)
    print("size", s.size())
    print("peek", s.peek())
    print("pop", s.pop())
    print("pop", s.pop())
    print("size after", s.size())

    # Stack with strings
    ss = Stack()
    ss.push("hello")
    ss.push("world")
    print("string pop", ss.pop())

    # Pair
    p = Pair("name", 42)
    print("pair key", p.key)
    print("pair value", p.value)
    swapped = p.swap()
    print("swapped key", swapped.key)
    print("swapped value", swapped.value)

    # Container
    c = Container("original")
    print("container get", c.get())
    c2 = c.replace("replaced")
    print("container replaced", c2.get())
    print("container original", c.get())

    # NumberStack (generic inheritance)
    ns = NumberStack()
    ns.push(10)
    ns.push(20)
    ns.push(30)
    print("number sum", ns.sum_all())
    print("number size", ns.size())

    # __type_params__ attribute (PEP 695)
    tp = getattr(Stack, "__type_params__", None)
    print("type_params exists", tp is not None)
    if tp is not None:
        print("type_params len", len(tp))
        print("type_param name", tp[0].__name__)

    # Pair type params
    ptp = getattr(Pair, "__type_params__", None)
    if ptp is not None:
        print("pair type_params len", len(ptp))
        names = [t.__name__ for t in ptp]
        print("pair type_param names", names)

    # Subscript syntax
    print("Stack[int]", Stack[int])
    print("Pair[str, int]", Pair[str, int])

    # Generic function (PEP 695)
    def first[T](items: list[T]) -> T:
        return items[0]

    print("generic func", first([10, 20, 30]))
    ftp = getattr(first, "__type_params__", None)
    if ftp is not None:
        print("func type_params len", len(ftp))
        print("func type_param name", ftp[0].__name__)

    # Type alias (PEP 695)
    type Matrix[T] = list[list[T]]
    print("type alias", Matrix[int])

    # isinstance still works with generic classes
    print("isinstance stack", isinstance(s, Stack))
    print("isinstance numberstack", isinstance(ns, NumberStack))
    print("isinstance numberstack is stack", isinstance(ns, Stack))
