def f():
    vals = []
    vals.append(10)
    vals.append(20)
    return (vals, 0)

result = f()
vals_from_result = result[0]
print(vals_from_result)
