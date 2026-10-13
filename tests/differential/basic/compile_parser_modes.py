"""Purpose: differential coverage for compile() parser-backed mode validation."""


def _ok(label: str, source: str, mode: str) -> None:
    code = compile(source, "<compile-parser-modes>", mode)
    print(label, type(code).__name__, code.co_name)


def _syntax_error(label: str, source: str, mode: str) -> None:
    try:
        compile(source, "<compile-parser-modes>", mode)
        print(label, "missed")
    except SyntaxError as exc:
        print(label, type(exc).__name__)


def main() -> None:
    _ok("ok-exec", "x = 1\ny = x + 2\n", "exec")
    _ok("ok-eval", "1 + 2", "eval")
    _ok("ok-single", "x = 1", "single")

    _syntax_error("bad-parse", "def f(:\n    pass\n", "exec")
    _syntax_error("bad-indent", "if True:\npass\n", "exec")


if __name__ == "__main__":
    main()


import builtins

print(
    "compile-named",
    compile(
        source="1 + 2",
        filename="named",
        mode="eval",
        flags=0,
        dont_inherit=False,
        optimize=0,
    ).co_name,
)
selected = builtins.compile
print(
    "compile-indirect",
    selected(**{"source": "3", "filename": "indirect", "mode": "eval"}).co_name,
)
print("compile-positional", selected("4", "positional", "eval", 0, False, 0).co_name)
for args, kwargs in [
    (("1", "x", "eval"), {"source": "2"}),
    (("1", "x", "eval"), {"bad": 1}),
]:
    try:
        selected(*args, **kwargs)
    except TypeError:
        print("compile-refusal", True)
    else:
        print("compile-refusal", False)
