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
