import re


m = re.search(r"(\w+)", "  hello  ")
assert m is not None
print("pos", m.pos)
print("endpos", m.endpos)
print("lastindex", m.lastindex)

m2 = re.match(r"(?P<name>\w+)", "hello")
assert m2 is not None
print("lastgroup", m2.lastgroup)
