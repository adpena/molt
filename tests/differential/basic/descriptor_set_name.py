class Log:
    def __init__(self, tag: str) -> None:
        self.tag = tag

    def __set_name__(self, owner, name: str) -> None:
        print(f"{self.tag}:{owner.__name__}.{name}")


shared = Log("shared")


class Widget:
    alpha = Log("alpha")
    beta = shared
    gamma = shared


print("done")
