"""Purpose: differential coverage for __set_name__ with inheritance order."""

log = []


class Recorder:
    def __init__(self, tag):
        self.tag = tag

    def __set_name__(self, owner, name):
        log.append((self.tag, owner.__name__, name))


class Base:
    base = Recorder("base")


class Mid(Base):
    mid = Recorder("mid")


class Child(Mid):
    child = Recorder("child")


if __name__ == "__main__":
    print("log", log)
