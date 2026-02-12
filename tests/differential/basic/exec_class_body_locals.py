"""Purpose: differential coverage for exec in class body locals mapping."""

class Demo:
    namespace = {}
    exec("x = 10", {}, namespace)


if __name__ == "__main__":
    print("class_has_x", hasattr(Demo, "x"))
    print("namespace", Demo.namespace.get("x"))
