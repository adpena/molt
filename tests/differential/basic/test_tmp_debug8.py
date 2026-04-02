def f():
    vals = []
    vals.append(10)
    vals.append(20)
    t = (vals, 0)
    print("inside:", t)
    return t

result = f()
print("outside:", result)
