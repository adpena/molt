"""Exception handlers validate real classes before matching actual inheritance."""
import builtins


def select(handler, error):
    try:
        raise error
    except handler:
        return "caught"


class CatchMeta(type):
    def __instancecheck__(self, instance):
        raise AssertionError("handler invoked metaclass __instancecheck__")


class Parent(Exception, metaclass=CatchMeta):
    pass


class Child(Parent):
    pass


print("subclass", select(Parent, Child()))
for handler in ((), (Parent,), (Parent, 17), ((Parent,),), Parent(), int):
    error = Child("original")
    try:
        print("handler", select(handler, error))
    except builtins.TypeError as caught:
        print("invalid", str(caught), caught.__context__ is error)
    except Child as caught:
        print("unmatched", caught is error)


ValueError = builtins.TypeError
print("rebound", select(ValueError, builtins.TypeError()))
for handler in ((), (ValueError,), (ValueError, int)):
    original = builtins.TypeError("original")
    try:
        try:
            raise original
        except* handler:
            print("star-caught")
    except builtins.TypeError as caught:
        print("star-unmatched", caught is original)
