"""Intrinsic-backed argparse subset for Molt.

Uses handle-based Rust intrinsics for all heavy parsing work.  The Python
shim is responsible only for argument normalization, type conversion, and
the public CPython-compatible API surface.
"""

from __future__ import annotations

import sys
from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = [
    "ArgumentError",
    "ArgumentParser",
    "FileType",
    "Namespace",
]

# ---------------------------------------------------------------------------
# Handle-based intrinsics
# ---------------------------------------------------------------------------
_PARSER_NEW = _require_intrinsic("molt_argparse_parser_new", globals())
_ADD_ARGUMENT = _require_intrinsic("molt_argparse_add_argument", globals())
_PARSE_ARGS = _require_intrinsic("molt_argparse_parse_args", globals())
_FORMAT_HELP = _require_intrinsic("molt_argparse_format_help", globals())
_FORMAT_USAGE = _require_intrinsic("molt_argparse_format_usage", globals())
_ERROR = _require_intrinsic("molt_argparse_error", globals())
_ADD_SUBPARSERS = _require_intrinsic("molt_argparse_add_subparsers", globals())
_ADD_PARSER = _require_intrinsic("molt_argparse_add_parser", globals())
_ADD_MUTUALLY_EXCLUSIVE = _require_intrinsic(
    "molt_argparse_add_mutually_exclusive", globals()
)
_GROUP_ADD_ARGUMENT = _require_intrinsic("molt_argparse_group_add_argument", globals())
_PARSER_DROP = _require_intrinsic("molt_argparse_parser_drop", globals())

_UNSET = object()


# ---------------------------------------------------------------------------
# Public exception
# ---------------------------------------------------------------------------
class ArgumentError(Exception):
    def __init__(self, argument_name: str | None, message: str) -> None:
        self.argument_name = argument_name
        self.message = message
        super().__init__(message)


# ---------------------------------------------------------------------------
# Namespace
# ---------------------------------------------------------------------------
class Namespace:
    def __init__(self, **kwargs: Any) -> None:
        self.__dict__.update(kwargs)


# ---------------------------------------------------------------------------
# FileType
# ---------------------------------------------------------------------------
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


# ---------------------------------------------------------------------------
# Converter bookkeeping — the Rust side stores type_name as a hint but
# does not apply Python-level type conversion.  We track converters
# per-dest so we can apply them after parse_args returns.
# ---------------------------------------------------------------------------
class _ConverterMap:
    """Tracks Python type-converters keyed by dest name."""

    __slots__ = ("_map",)

    def __init__(self) -> None:
        self._map: dict[str, Any] = {}

    def register(self, dest: str, converter: Any) -> None:
        if converter is not None:
            self._map[dest] = converter

    def apply(self, parsed: dict[str, Any]) -> None:
        for dest, conv in self._map.items():
            if dest not in parsed:
                continue
            value = parsed[dest]
            if value is None or not isinstance(value, str):
                continue
            try:
                parsed[dest] = conv(value)
            except Exception as exc:
                raise ArgumentError(dest, str(exc)) from exc


# ---------------------------------------------------------------------------
# _MutuallyExclusiveGroup
# ---------------------------------------------------------------------------
class _MutuallyExclusiveGroup:
    """Proxy returned by ``ArgumentParser.add_mutually_exclusive_group``."""

    __slots__ = ("_parser", "_group_handle", "_converters")

    def __init__(
        self,
        parser: ArgumentParser,
        group_handle: int,
        converters: _ConverterMap,
    ) -> None:
        self._parser = parser
        self._group_handle = group_handle
        self._converters = converters

    def add_argument(
        self,
        *name_or_flags: str,
        action: str | None = None,
        default: Any = _UNSET,
        type: Any = None,
        help: str | None = None,
        nargs: Any = None,
        **_kwargs: Any,
    ) -> None:
        if not name_or_flags:
            raise TypeError("add_argument requires at least one name")
        first = str(name_or_flags[0])
        dest = first.lstrip("-").replace("-", "_") if first.startswith("-") else first
        if action is None:
            action_str = None
        else:
            action_str = str(action)

        default_str: str | None = None
        if default is not _UNSET and default is not None:
            default_str = str(default)

        self._converters.register(dest, type)

        _GROUP_ADD_ARGUMENT(
            self._group_handle,
            first,
            nargs,
            default_str,
            None,  # type_name hint — Python converter handles this
            help,
            action_str,
        )


# ---------------------------------------------------------------------------
# _SubParsersAction
# ---------------------------------------------------------------------------
class _SubParsersAction:
    """Proxy returned by ``ArgumentParser.add_subparsers``."""

    __slots__ = ("_group_handle", "_exit_on_error", "_child_converters")

    def __init__(
        self,
        group_handle: int,
        exit_on_error: bool,
    ) -> None:
        self._group_handle = group_handle
        self._exit_on_error = exit_on_error
        self._child_converters: dict[str, _ConverterMap] = {}

    def add_parser(self, name: str, **_kwargs: Any) -> ArgumentParser:
        help_text = _kwargs.get("help")
        sub_handle = _ADD_PARSER(self._group_handle, str(name), help_text)
        parser = ArgumentParser.__new__(ArgumentParser)
        parser._handle = int(sub_handle)
        parser.exit_on_error = self._exit_on_error
        parser._converters = _ConverterMap()
        parser._subparsers = None
        parser._owns_handle = True
        self._child_converters[str(name)] = parser._converters
        return parser


# ---------------------------------------------------------------------------
# ArgumentParser
# ---------------------------------------------------------------------------
class ArgumentParser:
    """Handle-backed argument parser that delegates to Rust intrinsics."""

    def __init__(
        self,
        prog: str | None = None,
        description: str | None = None,
        epilog: str | None = None,
        *,
        exit_on_error: bool = True,
        **_kwargs: Any,
    ) -> None:
        self._handle: int = int(_PARSER_NEW(prog, description, epilog))
        self.exit_on_error: bool = bool(exit_on_error)
        self._converters: _ConverterMap = _ConverterMap()
        self._subparsers: _SubParsersAction | None = None
        self._owns_handle: bool = True

    # -- add_argument ------------------------------------------------------
    def add_argument(
        self,
        *name_or_flags: str,
        action: str | None = None,
        default: Any = _UNSET,
        type: Any = None,
        required: bool = False,
        dest: str | None = None,
        nargs: Any = None,
        help: str | None = None,
        choices: list[str] | None = None,
        **_kwargs: Any,
    ) -> None:
        if not name_or_flags:
            raise TypeError("add_argument requires at least one name")
        first = str(name_or_flags[0])

        # Compute dest for converter registration
        if dest is not None:
            target_dest = str(dest)
        elif first.startswith("-"):
            target_dest = first.lstrip("-").replace("-", "_")
        else:
            target_dest = first

        # Determine action string for intrinsic
        if action is None:
            action_str = None
        else:
            action_str = str(action)

        # Default handling
        default_str: str | None = None
        if default is not _UNSET and default is not None:
            default_str = str(default)

        # Register Python-level type converter
        if action_str != "store_true" and action_str != "store_false":
            self._converters.register(target_dest, type)

        _ADD_ARGUMENT(
            self._handle,
            first,  # name
            nargs,  # nargs (None, str, or int — Rust handles all)
            default_str,  # default as str or None
            None,  # type_name hint — Python converter handles this
            help,  # help text
            required,  # required flag
            action_str,  # action string
            dest,  # dest override or None
            choices,  # choices list or None
        )

    # -- add_subparsers ----------------------------------------------------
    def add_subparsers(
        self,
        *,
        title: str | None = None,
        dest: str | None = None,
        required: bool = False,
        **_kwargs: Any,
    ) -> _SubParsersAction:
        if self._subparsers is not None:
            raise RuntimeError("argparse currently supports only one subparsers group")
        group_handle = int(_ADD_SUBPARSERS(self._handle, title, dest))
        self._subparsers = _SubParsersAction(
            group_handle,
            exit_on_error=self.exit_on_error,
        )
        return self._subparsers

    # -- add_mutually_exclusive_group --------------------------------------
    def add_mutually_exclusive_group(
        self, *, required: bool = False
    ) -> _MutuallyExclusiveGroup:
        group_handle = int(_ADD_MUTUALLY_EXCLUSIVE(self._handle, required))
        return _MutuallyExclusiveGroup(self, group_handle, self._converters)

    # -- parse_args --------------------------------------------------------
    def parse_args(self, args: list[str] | None = None) -> Namespace:
        argv = list(sys.argv[1:] if args is None else args)
        for item in argv:
            if not isinstance(item, str):
                raise TypeError("argparse arguments must be str")
        try:
            parsed = _PARSE_ARGS(self._handle, argv)
        except (SystemExit, ValueError) as exc:
            if self.exit_on_error:
                raise
            raise ArgumentError(None, str(exc)) from exc
        if not isinstance(parsed, dict):
            raise RuntimeError("argparse.parse_args intrinsic returned invalid payload")

        # Apply Python-level type converters
        self._converters.apply(parsed)

        # If subparsers are active, apply child converters
        if self._subparsers is not None:
            for cmd_name, child_convs in self._subparsers._child_converters.items():
                # Check if parsed dict contains this subcommand
                for _key, val in parsed.items():
                    if val == cmd_name:
                        child_convs.apply(parsed)
                        break

        return Namespace(**parsed)

    # -- format_help / format_usage ----------------------------------------
    def format_help(self) -> str:
        return str(_FORMAT_HELP(self._handle))

    def format_usage(self) -> str:
        return str(_FORMAT_USAGE(self._handle))

    def print_help(self) -> None:
        print(self.format_help(), end="")

    def print_usage(self) -> None:
        print(self.format_usage(), end="")

    # -- error -------------------------------------------------------------
    def error(self, message: str) -> None:
        _ERROR(self._handle, message)

    # -- cleanup -----------------------------------------------------------
    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None and getattr(self, "_owns_handle", False):
            _PARSER_DROP(handle)
