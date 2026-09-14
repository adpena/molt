class MyTuple(tuple):
    pass

t = MyTuple((1, 2, 3))
print(t)
print(type(t).__name__)
print(len(t))

copied = MyTuple(t)
print(type(copied).__name__, copied, copied is t)
print(type(tuple(t)).__name__, tuple(t))
print(tuple.__len__(t), tuple.__getitem__(t, 1), tuple.__contains__(t, 2))
print(t.count(2), t.index(2, 0, 3))


class OverrideTuple(MyTuple):
    def __iter__(self):
        return iter((9,))


overridden = OverrideTuple((7,))
print(tuple(overridden), MyTuple(overridden))
print(list(tuple.__iter__(overridden)))
OverrideTuple.__iter__ = None
try:
    MyTuple(overridden)
except TypeError:
    print("disabled iteration rejected")
