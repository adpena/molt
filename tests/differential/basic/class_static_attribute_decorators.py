"""Purpose: decorators do not redefine CPython's syntactic class metadata."""
# MOLT_META: min_py=3.13


def passthrough(fn):
    return fn


class C:
    annotation: int

    @classmethod
    def class_method(self):
        self.class_field = 1

    @staticmethod
    def static_method(self):
        self.static_field = 1

    @passthrough
    def decorated(self):
        self.decorated_field = 1

    def other(first):
        first.not_literal_self = 1

    def nested(self):
        def inner():
            self.inner_field = 1

    def unpack(self):
        self.z, self.a = 1, 2

    class Inner:
        def inner(self):
            self.belongs_to_inner = 1


print(C.__static_attributes__)
print(C.Inner.__static_attributes__)
