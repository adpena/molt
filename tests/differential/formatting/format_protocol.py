class Custom:
    def __init__(self, tag):
        self.tag = tag

    def __format__(self, spec):
        return f"custom[{self.tag}:{spec}]"


class Plain:
    pass


class Box:
    def __init__(self):
        self.name = "molt"
        self.items = {"x": 2, "y": 3}
        self.seq = [10, 20]


print(f"{Custom('a'):q}")
print("{obj:q}".format(obj=Custom("b")))

try:
    print(f"{Plain():>5}")
except Exception as e:
    print(type(e).__name__, e)

box = Box()
print("{0.name} {0.items[x]} {0.seq[1]}".format(box))
print("{box.name} {box.items[y]} {box.seq[0]}".format(box=box))

print("{:n}".format(1234567))
print("{:n}".format(12345.67))
try:
    print("{:,n}".format(123))
except Exception as e:
    print(type(e).__name__, e)

spec = ">6"
print(f"{3:{spec}}")
width = 4
prec = 2
print(f"{12.345:{width}.{prec}f}")

print(format(7))
print(format(7, ">4"))
print(format(Custom("c"), "z"))
try:
    print("{:_n}".format(123))
except Exception as e:
    print(type(e).__name__, e)
