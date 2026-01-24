"""Purpose: differential coverage for formatting."""

name = "Molt"
count = 3
print(f"hello {name}")
print(f"{count}!")
print(f"{count:04d}")
print(f"{3.1:.2f}")
print("hi {} {}".format("a", 2))
print("value {0}".format(5))
print("name {name:>6}".format(name="x"))
print("brace {{}}".format())
