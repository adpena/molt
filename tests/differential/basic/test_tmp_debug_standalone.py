def f():
    vals = []
    vals.append(10)
    vals.append(20)
    print(vals)  # print list inside function
    return (vals, 0)

result = f()
print(result[0])  # print list outside function
