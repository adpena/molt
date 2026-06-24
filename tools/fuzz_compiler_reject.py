from __future__ import annotations

from random import Random

# ---------------------------------------------------------------------------
# Reject program generator
# ---------------------------------------------------------------------------


class RejectProgramGenerator:
    """Generates programs that use dynamic features Molt should reject."""

    REJECT_TEMPLATES: list[tuple[str, str]] = [
        # exec
        (
            'exec("x = 1 + 2")\nprint(x)',
            "exec",
        ),
        (
            'exec(compile("y = 42", "<string>", "exec"))\nprint(y)',
            "exec",
        ),
        # eval
        (
            'result = eval("1 + 2")\nprint(result)',
            "eval",
        ),
        (
            'vals = [1, 2, 3]\nresult = eval("sum(vals)")\nprint(result)',
            "eval",
        ),
        # setattr / delattr
        (
            "class Foo:\n    pass\nobj = Foo()\nsetattr(obj, 'x', 1)\nprint(obj.x)",
            "setattr",
        ),
        (
            "class Bar:\n    x = 10\nobj = Bar()\ndelattr(obj, 'x')",
            "delattr",
        ),
        # type() dynamic class creation
        (
            'MyClass = type("MyClass", (object,), {"value": 42})\nobj = MyClass()\nprint(obj.value)',
            "dynamic type()",
        ),
        # __dict__ mutation
        (
            "class Baz:\n    pass\nobj = Baz()\nobj.__dict__['secret'] = 99\nprint(obj.secret)",
            "__dict__ mutation",
        ),
        # Monkeypatching
        (
            "class Animal:\n    pass\nAnimal.speak = lambda self: 'woof'\ndog = Animal()\nprint(dog.speak())",
            "monkeypatch",
        ),
        (
            "class Base:\n    def greet(self):\n        return 'hi'\nBase.greet = lambda self: 'bye'\nprint(Base().greet())",
            "monkeypatch",
        ),
        # globals() / locals() mutation
        (
            'globals()["dynamic_var"] = 100\nprint(dynamic_var)',
            "globals mutation",
        ),
        (
            'def f():\n    locals()["x"] = 5\n    return x\nprint(f())',
            "locals mutation",
        ),
        # getattr with runtime-determined name
        (
            "class Obj:\n    x = 1\n    y = 2\nimport random\nattr_name = ['x', 'y'][0]\nobj = Obj()\nprint(getattr(obj, attr_name))",
            "dynamic getattr",
        ),
        # __class__ assignment
        (
            "class A:\n    pass\nclass B:\n    pass\na = A()\na.__class__ = B\nprint(type(a).__name__)",
            "__class__ assignment",
        ),
        # __bases__ mutation
        (
            "class X:\n    pass\nclass Y:\n    pass\nX.__bases__ = (Y,)",
            "__bases__ mutation",
        ),
    ]

    def __init__(self, rng: Random):
        self.rng = rng

    def generate(self) -> tuple[str, str]:
        """Returns (source, expected_rejection_reason)."""
        source, reason = self.rng.choice(self.REJECT_TEMPLATES)
        return source + "\n", reason
