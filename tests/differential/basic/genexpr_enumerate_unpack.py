items = ("a", "b", "c")
result = {k: v for k, v in enumerate(items)}
print(result)

result2 = [(i, x) for i, x in enumerate([10, 20, 30])]
print(result2)
