"""Command interpreter framework (CPython-compatible core surface)."""

from __future__ import annotations

import importlib.util as _importlib_util
import sys
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic(
    "molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()


PROMPT = "(Cmd) "
IDENTCHARS = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_"


def _load_readline_module():
    if _importlib_util.find_spec("readline") is None:
        return None
    return __import__("readline")


class Cmd:
    prompt = PROMPT
    identchars = IDENTCHARS
    ruler = "="
    lastcmd = ""
    intro = None
    doc_leader = ""
    doc_header = "Documented commands (type help <topic>):"
    misc_header = "Miscellaneous help topics:"
    undoc_header = "Undocumented commands:"
    nohelp = "*** No help on %s"
    use_rawinput = 1

    def __init__(
        self, completekey: str | None = "tab", stdin=None, stdout=None
    ) -> None:
        self.stdin = sys.stdin if stdin is None else stdin
        self.stdout = sys.stdout if stdout is None else stdout
        self.cmdqueue: list[str] = []
        self.completekey = completekey

    def cmdloop(self, intro: str | None = None) -> None:
        self.preloop()
        if self.use_rawinput and self.completekey:
            readline = _load_readline_module()
            if readline is not None:
                self.old_completer = readline.get_completer()
                readline.set_completer(self.complete)
                if getattr(readline, "backend", "") == "editline":
                    if self.completekey == "tab":
                        command_string = "bind ^I rl_complete"
                    else:
                        command_string = f"bind {self.completekey} rl_complete"
                else:
                    command_string = f"{self.completekey}: complete"
                readline.parse_and_bind(command_string)
        try:
            if intro is not None:
                self.intro = intro
            if self.intro:
                self.stdout.write(f"{self.intro}\n")
            stop: bool | None = None
            while not stop:
                if self.cmdqueue:
                    line = self.cmdqueue.pop(0)
                else:
                    if self.use_rawinput:
                        try:
                            line = input(self.prompt)
                        except EOFError:
                            line = "EOF"
                    else:
                        self.stdout.write(self.prompt)
                        self.stdout.flush()
                        line = self.stdin.readline()
                        if not line:
                            line = "EOF"
                        else:
                            line = line.rstrip("\r\n")
                line = self.precmd(line)
                stop = self.onecmd(line)
                stop = self.postcmd(stop, line)
            self.postloop()
        finally:
            if self.use_rawinput and self.completekey:
                readline = _load_readline_module()
                if readline is not None and hasattr(self, "old_completer"):
                    readline.set_completer(self.old_completer)

    def precmd(self, line: str) -> str:
        return line

    def postcmd(self, stop: bool | None, line: str) -> bool | None:
        return stop

    def preloop(self) -> None:
        pass

    def postloop(self) -> None:
        pass

    def parseline(self, line: str) -> tuple[str | None, str | None, str]:
        line = line.strip()
        if not line:
            return None, None, line
        if line[0] == "?":
            line = "help " + line[1:]
        elif line[0] == "!":
            if hasattr(self, "do_shell"):
                line = "shell " + line[1:]
            else:
                return None, None, line
        i, n = 0, len(line)
        while i < n and line[i] in self.identchars:
            i += 1
        cmd_name, arg = line[:i], line[i:].strip()
        return cmd_name, arg, line

    def onecmd(self, line: str) -> bool | None:
        cmd_name, arg, line = self.parseline(line)
        if not line:
            return self.emptyline()
        if cmd_name is None:
            return self.default(line)
        self.lastcmd = line
        if line == "EOF":
            self.lastcmd = ""
        if cmd_name == "":
            return self.default(line)
        func = getattr(self, "do_" + cmd_name, None)
        if func is None:
            return self.default(line)
        return func(arg)

    def emptyline(self) -> bool | None:
        if self.lastcmd:
            return self.onecmd(self.lastcmd)
        return None

    def default(self, line: str) -> None:
        self.stdout.write(f"*** Unknown syntax: {line}\n")
        return None

    def completedefault(self, *ignored: Any) -> list[str]:
        return []

    def completenames(self, text: str, *ignored: Any) -> list[str]:
        dotext = "do_" + text
        return [name[3:] for name in self.get_names() if name.startswith(dotext)]

    def complete(self, text: str, state: int):
        if state == 0:
            readline = _load_readline_module()
            if readline is None:
                self.completion_matches = []
                return None

            origline = readline.get_line_buffer()
            line = origline.lstrip()
            stripped = len(origline) - len(line)
            begidx = readline.get_begidx() - stripped
            endidx = readline.get_endidx() - stripped
            if begidx > 0:
                cmd_name, _args, _line = self.parseline(line)
                if not cmd_name:
                    compfunc = self.completedefault
                else:
                    compfunc = getattr(
                        self, "complete_" + cmd_name, self.completedefault
                    )
            else:
                compfunc = self.completenames
            self.completion_matches = compfunc(text, line, begidx, endidx)
        try:
            return self.completion_matches[state]
        except IndexError:
            return None

    def get_names(self) -> list[str]:
        return dir(self.__class__)

    def complete_help(self, *args: Any) -> list[str]:
        commands = set(self.completenames(*args))
        topics = set(
            name[5:] for name in self.get_names() if name.startswith("help_" + args[0])
        )
        return list(commands | topics)

    def do_help(self, arg: str) -> None:
        if arg:
            func = getattr(self, "help_" + arg, None)
            if func is None:
                from inspect import cleandoc

                do_cmd = getattr(self, "do_" + arg, None)
                if do_cmd is not None:
                    doc = cleandoc(do_cmd.__doc__ or "")
                    if doc:
                        self.stdout.write(f"{doc}\n")
                        return
                self.stdout.write(f"{self.nohelp % (arg,)}\n")
                return
            func()
            return

        names = self.get_names()
        cmds_doc: list[str] = []
        cmds_undoc: list[str] = []
        topics = {name[5:] for name in names if name.startswith("help_")}
        names.sort()
        prevname = ""
        for name in names:
            if not name.startswith("do_"):
                continue
            if name == prevname:
                continue
            prevname = name
            cmd_name = name[3:]
            if cmd_name in topics:
                cmds_doc.append(cmd_name)
                topics.remove(cmd_name)
            elif getattr(self, name).__doc__:
                cmds_doc.append(cmd_name)
            else:
                cmds_undoc.append(cmd_name)

        self.stdout.write(f"{self.doc_leader}\n")
        self.print_topics(self.doc_header, cmds_doc, 15, 80)
        self.print_topics(self.misc_header, sorted(topics), 15, 80)
        self.print_topics(self.undoc_header, cmds_undoc, 15, 80)

    def print_topics(
        self,
        header: str,
        cmds: list[str],
        cmdlen: int,
        maxcol: int,
    ) -> None:
        _ = cmdlen
        if not cmds:
            return
        self.stdout.write(f"{header}\n")
        if self.ruler:
            self.stdout.write(f"{self.ruler * len(header)}\n")
        self.columnize(cmds, maxcol - 1)
        self.stdout.write("\n")

    def columnize(self, items: list[str], displaywidth: int = 80) -> None:
        if not items:
            self.stdout.write("<empty>\n")
            return
        nonstrings = [i for i in range(len(items)) if not isinstance(items[i], str)]
        if nonstrings:
            bad = ", ".join(map(str, nonstrings))
            raise TypeError(f"list[i] not a string for i in {bad}")

        size = len(items)
        if size == 1:
            self.stdout.write(f"{items[0]}\n")
            return

        for nrows in range(1, size):
            ncols = (size + nrows - 1) // nrows
            colwidths: list[int] = []
            totwidth = -2
            for col in range(ncols):
                colwidth = 0
                for row in range(nrows):
                    i = row + nrows * col
                    if i >= size:
                        break
                    colwidth = max(colwidth, len(items[i]))
                colwidths.append(colwidth)
                totwidth += colwidth + 2
                if totwidth > displaywidth:
                    break
            if totwidth <= displaywidth:
                break
        else:
            nrows = size
            ncols = 1
            colwidths = [0]

        for row in range(nrows):
            texts: list[str] = []
            for col in range(ncols):
                i = row + nrows * col
                texts.append("" if i >= size else items[i])
            while texts and not texts[-1]:
                texts.pop()
            for col in range(len(texts)):
                texts[col] = texts[col].ljust(colwidths[col])
            self.stdout.write("  ".join(texts) + "\n")


__all__ = ["Cmd", "PROMPT", "IDENTCHARS"]

globals().pop("_require_intrinsic", None)
