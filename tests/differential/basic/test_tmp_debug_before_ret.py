def f():
    vals = []
    vals.append(10)
    vals.append(20)
    t = (vals, 0)
    print("before ret:", vals[0], vals[1])
    return t

result = f()
print("after ret:", result)
