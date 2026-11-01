"""No source import may hide object reduction's implicit copyreg dependency."""
class Plain:
    def __init__(self):
        self.value = 1

value = Plain()
for protocol in range(6):
    rv = value.__reduce_ex__(protocol)
    rebuilt = rv[0](*rv[1])
    print(protocol, len(rv), rv[1][0] is Plain, rv[2] == {"value": 1},
          type(rebuilt) is Plain, rebuilt is not value)
