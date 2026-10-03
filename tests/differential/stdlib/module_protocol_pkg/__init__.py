"""Import fixtures whose parent observes child publication."""

import sys
import types

events = []
mode = "normal"


class Parent(types.ModuleType):
    def __setattr__(self, name, value):
        if name.startswith("child_"):
            events.append(("set", name))
            if mode == "attribute":
                raise AttributeError("publication rejected")
            if mode == "value":
                raise ValueError("publication rejected")
            if mode == "delete":
                del sys.modules[value.__name__]
            if mode == "replace":
                replacement = types.ModuleType(value.__name__)
                replacement.value = -1
                sys.modules[value.__name__] = replacement
        super().__setattr__(name, value)

    @property
    def child_descriptor(self):
        return self.__dict__["_descriptor_child"]

    @child_descriptor.setter
    def child_descriptor(self, value):
        events.append(("descriptor", "child_descriptor"))
        self.__dict__["_descriptor_child"] = value

    def __getattr__(self, name):
        if name == "virtual":
            return 73
        raise AttributeError(name)


sys.modules[__name__].__class__ = Parent
