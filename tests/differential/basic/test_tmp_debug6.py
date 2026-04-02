def f():
    vals = []
    vals.append(10)
    vals.append(20)
    return (vals, 0)

result = f()
# The list should still be alive via the tuple
print(result[0])  # the list
print(result[0][0])  # first element
