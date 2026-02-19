"""Intrinsic-backed argparse subset for Molt."""

from __future__ import annotations

import json
import sys
from dataclasses import dataclass
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "ArgumentError",
    "ArgumentParser",
    "FileType",
    "Namespace",
]

_MOLT_ARGPARSE_PARSE = _require_intrinsic("molt_argparse_parse", globals())

_UNSET = object()


class ArgumentError(Exception):
    def __init__(self, argument_name: str | None, message: str) -> None:
        self.argument_name = argument_name
        self.message = message
        super().__init__(message)


class Namespace:
    def __init__(self, **kwargs: Any) -> None:
        self.__dict__.update(kwargs)


class FileType:
    def __init__(
        self,
        mode: str = "r",
        bufsize: int = -1,
        encoding: str | None = None,
        errors: str | None = None,
    ) -> None:
        self._mode = mode
        self._bufsize = int(bufsize)
        self._encoding = encoding
        self._errors = errors

    def __call__(self, path: str):
        kwargs: dict[str, Any] = {}
        if "b" not in self._mode:
            if self._encoding is not None:
                kwargs["encoding"] = self._encoding
            if self._errors is not None:
                kwargs["errors"] = self._errors
        return open(path, self._mode, self._bufsize, **kwargs)


@dataclass(slots=True)
class _Argument:
    kind: str
    dest: str
    flag: str | None = None
    action: str | None = None
    required: bool = False
    default: Any = None
    converter: Any = None


class _SubParsersAction:
    def __init__(
        self,
        owner: "ArgumentParser",
        *,
        dest: str | None,
        required: bool,
    ) -> None:
        self._owner = owner
        self.dest = "command" if dest is None else str(dest)
        self.required = bool(required)
        self._parsers: dict[str, ArgumentParser] = {}

    def add_parser(self, name: str, **_kwargs: Any) -> "ArgumentParser":
        key = str(name)
        parser = ArgumentParser(exit_on_error=self._owner.exit_on_error)
        self._parsers[key] = parser
        return parser


class ArgumentParser:
    def __init__(self, *, exit_on_error: bool = True, **_kwargs: Any) -> None:
        self.exit_on_error = bool(exit_on_error)
        self._arguments: list[_Argument] = []
        self._subparsers: _SubParsersAction | None = None

    def add_argument(
        self,
        *name_or_flags: str,
        action: str | None = None,
        default: Any = _UNSET,
        type: Any = None,
        required: bool = False,
        dest: str | None = None,
        **_kwargs: Any,
    ):
        if not name_or_flags:
            raise TypeError("add_argument requires at least one name")
        first = str(name_or_flags[0])
        if first.startswith("-"):
            if action not in (None, "store_true"):
                raise RuntimeError(f'unsupported argparse action "{action}"')
            flag = first
            target_dest = (
                dest if dest is not None else first.lstrip("-").replace("-", "_")
            )
            is_store_true = action == "store_true"
            default_value = (
                False
                if is_store_true and default is _UNSET
                else None
                if default is _UNSET
                else default
            )
            arg = _Argument(
                kind="optional",
                flag=flag,
                dest=str(target_dest),
                action="store_true" if is_store_true else "value",
                required=bool(required),
                default=default_value,
                converter=type,
            )
            self._arguments.append(arg)
            return arg
        if action is not None:
            raise RuntimeError(f'unsupported positional action "{action}"')
        target_dest = first if dest is None else str(dest)
        arg = _Argument(
            kind="positional",
            dest=str(target_dest),
            default=None if default is _UNSET else default,
            converter=type,
        )
        self._arguments.append(arg)
        return arg

    def add_subparsers(
        self,
        *,
        dest: str | None = None,
        required: bool = False,
        **_kwargs: Any,
    ) -> _SubParsersAction:
        if self._subparsers is not None:
            raise RuntimeError("argparse currently supports only one subparsers group")
        self._subparsers = _SubParsersAction(self, dest=dest, required=required)
        return self._subparsers

    def _to_intrinsic_spec(self) -> dict[str, Any]:
        optionals: list[dict[str, Any]] = []
        positionals: list[dict[str, Any]] = []
        for arg in self._arguments:
            if arg.kind == "optional":
                optionals.append(
                    {
                        "flag": arg.flag,
                        "dest": arg.dest,
                        "kind": arg.action,
                        "required": arg.required,
                        "default": arg.default,
                    }
                )
            elif arg.kind == "positional":
                positionals.append({"dest": arg.dest})
        out: dict[str, Any] = {
            "optionals": optionals,
            "positionals": positionals,
        }
        if self._subparsers is not None:
            out["subparsers"] = {
                "dest": self._subparsers.dest,
                "required": self._subparsers.required,
                "parsers": {
                    name: parser._to_intrinsic_spec()
                    for name, parser in self._subparsers._parsers.items()
                },
            }
        return out

    def _apply_converters(self, parsed: dict[str, Any]) -> None:
        for arg in self._arguments:
            if arg.kind not in ("optional", "positional"):
                continue
            if arg.action == "store_true":
                continue
            if arg.dest not in parsed:
                continue
            value = parsed[arg.dest]
            if value is None or arg.converter is None or not isinstance(value, str):
                continue
            try:
                parsed[arg.dest] = arg.converter(value)
            except Exception as exc:  # pragma: no cover - mapped for parity behavior.
                raise ArgumentError(arg.dest, str(exc)) from exc
        if self._subparsers is None:
            return
        cmd = parsed.get(self._subparsers.dest)
        if isinstance(cmd, str):
            child = self._subparsers._parsers.get(cmd)
            if child is not None:
                child._apply_converters(parsed)

    def parse_args(self, args: list[str] | None = None) -> Namespace:
        argv = list(sys.argv[1:] if args is None else args)
        for item in argv:
            if not isinstance(item, str):
                raise TypeError("argparse arguments must be str")
        spec_json = json.dumps(self._to_intrinsic_spec(), separators=(",", ":"))
        try:
            payload = _MOLT_ARGPARSE_PARSE(spec_json, argv)
        except ValueError as exc:
            if self.exit_on_error:
                raise
            raise ArgumentError(None, str(exc)) from exc
        if not isinstance(payload, str):
            raise RuntimeError("argparse.parse intrinsic returned invalid payload")
        decoded = json.loads(payload)
        if not isinstance(decoded, dict):
            raise RuntimeError("argparse.parse intrinsic returned invalid payload")
        self._apply_converters(decoded)
        return Namespace(**decoded)
