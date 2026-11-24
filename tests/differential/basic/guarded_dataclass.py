from dataclasses import dataclass

@dataclass
class Record:
    value: int

def make(get_value):
    return Record(get_value())

def first():
    return 11

print("ordinary", make(first).value)
allocations = []
selected = []

def replaced(self, value):
    selected.append((id(self) == allocations[-1], value))
    return 1

def allocate(cls, value):
    instance = object.__new__(cls)
    allocations.append(id(instance))
    cls.__init__ = replaced
    return instance

def swap(callable_class, allocator):
    callable_class.__new__ = staticmethod(allocator)

swap(Record, allocate)
try:
    make(first)
except TypeError as exc:
    print("after allocation", len(allocations), selected, str(exc))

def rebind(value):
    global Record
    Record = value

def alternate(x):
    return ("rebound", x)

rebind(alternate)
print("rebound", make(first))
