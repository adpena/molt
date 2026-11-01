"""copy.replace belongs to the target Python 3.13+ surface (HF-649)."""
import copy
import sys

modern = sys.version_info >= (3, 13)
print("target surface", modern, hasattr(copy, "replace"), "replace" in copy.__all__)


class Record:
    def __init__(self, value):
        self.value = value

    def __replace__(self, **changes):
        return Record(changes.get("value", self.value))


if modern:
    original = Record(1)
    replaced = copy.replace(original, value=2)
    print("replace hook", original.value, replaced.value, replaced is not original)
