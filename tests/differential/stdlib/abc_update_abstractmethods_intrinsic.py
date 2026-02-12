"""Purpose: verify abc.update_abstractmethods lowers through runtime intrinsics."""

import abc


class Base(abc.ABC):
    @abc.abstractmethod
    def run(self):
        raise NotImplementedError


class Child(Base):
    pass


print("before", sorted(Child.__abstractmethods__))


def _impl(self):
    return "ok"


Child.run = _impl
updated = abc.update_abstractmethods(Child)
print("identity", updated is Child)
print("after", sorted(Child.__abstractmethods__))
print("call", Child().run())


class Plain:
    pass


print("plain", abc.update_abstractmethods(Plain) is Plain)
