"""Purpose: differential coverage for exec in class body."""

class Demo:
    exec("x = 1")
    exec("y = x + 1")


if __name__ == "__main__":
    print("attrs", Demo.x, Demo.y)
