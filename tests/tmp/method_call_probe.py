class Base:
    pass


class Foo(Base):
    def __init__(self) -> None:
        self._start_threads()

    def _start_threads(self) -> None:
        print("probe: start_threads called")


Foo()
print("probe: done")
