"""Local generator expressions use the yielded value and the throw protocol."""


def first_item():
    stream = (item for item in (41,))
    return stream.send(None)


def exhausted():
    stream = (item for item in ())
    return stream.send(None)


def throw_into_expression(error):
    stream = (item for item in ())
    stream.throw(ValueError, error)


print("yield", first_item())
try:
    exhausted()
except StopIteration as stopped:
    print("stop", stopped.value, stopped.args)

original = ValueError("original")
try:
    throw_into_expression(original)
except ValueError as caught:
    print("throw", caught is original, caught.args)
