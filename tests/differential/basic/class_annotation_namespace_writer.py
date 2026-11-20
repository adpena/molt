"""PEP 649 evaluators read a class's live mapping when they run: a name the
class body never bound resolves as a global, unless a store on the class
object (`C.x = v`, `setattr`) added it after the class existed."""
# MOLT_META: min_py=3.14

injected = "global"


class Closed:
    observed: injected


class Written:
    observed: injected


class ViaSetattr:
    observed: injected


class Box[T]:
    value: T


Written.injected = "class attribute"
setattr(ViaSetattr, "injected", "setattr attribute")
Box.T = "shadowing T"
print(Closed.__annotations__)
print(Written.__annotations__)
print(ViaSetattr.__annotations__)
print(Box.__annotations__)
