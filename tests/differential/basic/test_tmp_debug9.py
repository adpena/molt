def f():
    vals = []
    vals.append(10)
    vals.append(20)
    t = (vals, 0)
    return t

# Call twice to see if second call overwrites first's memory
r1 = f()
r2 = f()
print("r1:", r1)
print("r2:", r2)
