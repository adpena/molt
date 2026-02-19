"""Interactive interpreter support."""

from __future__ import annotations

import codeop
import sys
import traceback

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready", globals()
)
_MOLT_IMPORT_SMOKE_RUNTIME_READY()

compile_command = codeop.compile_command


class InteractiveInterpreter:
    def __init__(self, locals=None) -> None:
        if locals is None:
            locals = {"__name__": "__console__", "__doc__": None}
        self.locals = locals
        compiler_ctor = getattr(codeop, "CommandCompiler", None)
        if compiler_ctor is not None and callable(compiler_ctor):
            self.compile = compiler_ctor()
        else:
            self.compile = compile_command

    def runsource(
        self,
        source: str,
        filename: str = "<input>",
        symbol: str = "single",
    ) -> bool:
        try:
            code_obj = self.compile(source, filename, symbol)
        except (OverflowError, SyntaxError, ValueError):
            self.showsyntaxerror(filename, source=source)
            return False
        if code_obj is None:
            return True
        self.runcode(code_obj)
        return False

    def runcode(self, code_obj) -> None:
        try:
            exec(code_obj, self.locals)
        except SystemExit:
            raise
        except BaseException:
            self.showtraceback()

    def showsyntaxerror(self, filename: str | None = None, **kwargs) -> None:
        try:
            typ, value, _tb = sys.exc_info()
            if filename and typ is not None and issubclass(typ, SyntaxError):
                value.filename = filename
            source = kwargs.pop("source", "")
            self._showtraceback(typ, value, None, source)
        finally:
            typ = value = _tb = None

    def showtraceback(self) -> None:
        try:
            typ, value, tb = sys.exc_info()
            self._showtraceback(typ, value, tb.tb_next if tb is not None else None, "")
        finally:
            typ = value = tb = None

    def _showtraceback(self, typ, value, tb, source: str) -> None:
        sys.last_type = typ
        sys.last_traceback = tb
        value = value.with_traceback(tb)
        lines = source.splitlines()
        if (
            source
            and typ is SyntaxError
            and not value.text
            and value.lineno is not None
            and len(lines) >= value.lineno
        ):
            value.text = lines[value.lineno - 1]
        sys.last_exc = sys.last_value = value
        if sys.excepthook is sys.__excepthook__:
            self._excepthook(typ, value, tb)
            return
        try:
            sys.excepthook(typ, value, tb)
        except SystemExit:
            raise
        except BaseException as hook_exc:
            hook_exc.__context__ = None
            hook_exc = hook_exc.with_traceback(hook_exc.__traceback__.tb_next)
            print("Error in sys.excepthook:", file=sys.stderr)
            sys.__excepthook__(type(hook_exc), hook_exc, hook_exc.__traceback__)
            print(file=sys.stderr)
            print("Original exception was:", file=sys.stderr)
            sys.__excepthook__(typ, value, tb)

    def _excepthook(self, typ, value, tb) -> None:
        lines = traceback.format_exception(typ, value, tb)
        self.write("".join(lines))

    def write(self, data: str) -> None:
        sys.stderr.write(data)


class InteractiveConsole(InteractiveInterpreter):
    def __init__(
        self, locals=None, filename: str = "<console>", *, local_exit: bool = False
    ):
        super().__init__(locals)
        self.filename = filename
        self.local_exit = local_exit
        self.resetbuffer()

    def resetbuffer(self) -> None:
        self.buffer: list[str] = []

    def interact(self, banner: str | None = None, exitmsg: str | None = None) -> None:
        try:
            sys.ps1
        except AttributeError:
            sys.ps1 = ">>> "
        try:
            sys.ps2
        except AttributeError:
            sys.ps2 = "... "

        if banner:
            self.write(f"{banner}\n")

        more = False
        while True:
            try:
                prompt = sys.ps2 if more else sys.ps1
                line = self.raw_input(prompt)
            except EOFError:
                self.write("\n")
                break
            except KeyboardInterrupt:
                self.write("\nKeyboardInterrupt\n")
                self.resetbuffer()
                more = False
                continue
            more = bool(self.push(line))

        if exitmsg is None:
            self.write(f"now exiting {self.__class__.__name__}...\n")
        elif exitmsg != "":
            self.write(f"{exitmsg}\n")

    def push(
        self, line: str, filename: str | None = None, _symbol: str = "single"
    ) -> bool:
        self.buffer.append(line)
        source = "\n".join(self.buffer)
        if filename is None:
            filename = self.filename
        more = self.runsource(source, filename, symbol=_symbol)
        if not more:
            self.resetbuffer()
        return bool(more)

    def raw_input(self, prompt: str = "") -> str:
        return input(prompt)


def interact(
    banner: str | None = None,
    readfunc=None,
    local: dict[str, object] | None = None,
    exitmsg: str | None = None,
) -> None:
    console = InteractiveConsole(local)
    if readfunc is not None:
        console.raw_input = readfunc  # type: ignore[assignment]
    console.interact(banner=banner, exitmsg=exitmsg)


__all__ = [
    "InteractiveInterpreter",
    "InteractiveConsole",
    "compile_command",
    "interact",
]
