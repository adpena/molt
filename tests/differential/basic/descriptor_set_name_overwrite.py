"""Purpose: differential coverage for __set_name__ overwrite behavior."""

log = []


class Recorder:
    def __init__(self, tag):
        self.tag = tag

    def __set_name__(self, owner, name):
        log.append((self.tag, owner.__name__, name))


class Demo:
    first = Recorder("first")
    first = Recorder("second")


if __name__ == "__main__":
    print("log", log)
