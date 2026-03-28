class MyTuple(tuple):
    pass

t = MyTuple((1, 2, 3))
print(t)
print(type(t).__name__)
print(len(t))
