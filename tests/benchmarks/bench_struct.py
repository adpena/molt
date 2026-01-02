class Point:
    x: int
    y: int


i = 0
while i < 1000000:
    p = Point()
    p.x = i
    p.y = i + 1
    i = i + 1
print(i)
