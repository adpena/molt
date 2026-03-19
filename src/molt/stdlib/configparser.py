"""Intrinsic-backed configparser module for Molt — fully intrinsic-backed."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_molt_configparser_new = _require_intrinsic("molt_configparser_new")
_molt_configparser_read_string = _require_intrinsic(
    "molt_configparser_read_string"
)
_molt_configparser_read = _require_intrinsic("molt_configparser_read")
_molt_configparser_sections = _require_intrinsic(
    "molt_configparser_sections"
)
_molt_configparser_has_section = _require_intrinsic(
    "molt_configparser_has_section"
)
_molt_configparser_has_option = _require_intrinsic(
    "molt_configparser_has_option"
)
_molt_configparser_get = _require_intrinsic("molt_configparser_get")
_molt_configparser_getint = _require_intrinsic("molt_configparser_getint")
_molt_configparser_getfloat = _require_intrinsic(
    "molt_configparser_getfloat"
)
_molt_configparser_getboolean = _require_intrinsic(
    "molt_configparser_getboolean"
)
_molt_configparser_options = _require_intrinsic("molt_configparser_options")
_molt_configparser_items = _require_intrinsic("molt_configparser_items")
_molt_configparser_set = _require_intrinsic("molt_configparser_set")
_molt_configparser_add_section = _require_intrinsic(
    "molt_configparser_add_section"
)
_molt_configparser_remove_section = _require_intrinsic(
    "molt_configparser_remove_section"
)
_molt_configparser_remove_option = _require_intrinsic(
    "molt_configparser_remove_option"
)
_molt_configparser_write = _require_intrinsic("molt_configparser_write")
_molt_configparser_drop = _require_intrinsic("molt_configparser_drop")
_molt_configparser_write_string = _require_intrinsic(
    "molt_configparser_write_string"
)
_molt_configparser_get_raw = _require_intrinsic(
    "molt_configparser_get_raw"
)
_molt_configparser_interpolate_basic = _require_intrinsic(
    "molt_configparser_interpolate_basic"
)
_molt_configparser_interpolate_extended = _require_intrinsic(
    "molt_configparser_interpolate_extended"
)
_molt_configparser_read_file = _require_intrinsic(
    "molt_configparser_read_file"
)
_molt_configparser_defaults = _require_intrinsic(
    "molt_configparser_defaults"
)

_MISSING = object()


__all__ = [
    "BasicInterpolation",
    "ConfigParser",
    "DuplicateOptionError",
    "DuplicateSectionError",
    "Error",
    "ExtendedInterpolation",
    "Interpolation",
    "MissingSectionHeaderError",
    "NoOptionError",
    "NoSectionError",
    "ParsingError",
    "RawConfigParser",
]


# --- Exceptions ---


class Error(Exception):
    """Base class for ConfigParser exceptions."""

    def __init__(self, msg: str = "") -> None:
        self.message = msg
        super().__init__(msg)


class NoSectionError(Error):
    """Raised when a section is not found."""

    def __init__(self, section: str) -> None:
        self.section = section
        super().__init__(f"No section: {section!r}")


class NoOptionError(Error):
    """Raised when an option is not found in a section."""

    def __init__(self, option: str, section: str) -> None:
        self.option = option
        self.section = section
        super().__init__(f"No option {option!r} in section: {section!r}")


class DuplicateSectionError(Error):
    """Raised when a section is repeated in an input source."""

    def __init__(
        self,
        section: str,
        source: str | None = None,
        lineno: int | None = None,
    ) -> None:
        self.section = section
        self.source = source
        self.lineno = lineno
        msg = f"Section {section!r} already exists"
        if source is not None:
            msg = f"While reading from {source!r}"
            if lineno is not None:
                msg += f" [line {lineno:2d}]"
            msg += f": section {section!r} already exists"
        super().__init__(msg)


class DuplicateOptionError(Error):
    """Raised when an option is repeated in an input source."""

    def __init__(
        self,
        section: str,
        option: str,
        source: str | None = None,
        lineno: int | None = None,
    ) -> None:
        self.section = section
        self.option = option
        self.source = source
        self.lineno = lineno
        msg = f"Option {option!r} in section {section!r} already exists"
        if source is not None:
            msg = f"While reading from {source!r}"
            if lineno is not None:
                msg += f" [line {lineno:2d}]"
            msg += f": option {option!r} in section {section!r} already exists"
        super().__init__(msg)


class ParsingError(Error):
    """Raised when a configuration file cannot be parsed."""

    def __init__(self, source: str = "", filename: str | None = None) -> None:
        if filename is not None:
            source = filename
        self.source = source
        self.errors: list[tuple[int, str]] = []
        super().__init__(f"Source contains parsing errors: {source!r}")


class MissingSectionHeaderError(ParsingError):
    """Raised when a key-value pair is found before any section header."""

    def __init__(self, filename: str, lineno: int, line: str) -> None:
        self.filename = filename
        self.lineno = lineno
        self.line = line
        super().__init__(
            f"File contains no section headers.\nfile: {filename!r}, "
            f"line: {lineno}\n{line!r}"
        )


# --- Interpolation ---


class Interpolation:
    """Dummy interpolation base that performs no interpolation."""

    def before_get(
        self, parser: Any, section: str, option: str, value: str, defaults: Any
    ) -> str:
        return value

    def before_set(self, parser: Any, section: str, option: str, value: str) -> str:
        return value

    def before_read(self, parser: Any, section: str, option: str, value: str) -> str:
        return value

    def before_write(self, parser: Any, section: str, option: str, value: str) -> str:
        return value


class BasicInterpolation(Interpolation):
    """Interpolation as implemented in the classic ConfigParser.

    The option values can contain format strings which refer to other values
    in the same section, or values in the special default section.

    For example:

        something: %(dir)s/whatever

    would resolve the "%(dir)s" to the value of dir.  All reference
    expansions are done late, on demand. If a user needs to store a bare %
    in a configuration file, she can escape it by writing %%.
    """

    _MAX_INTERPOLATION_DEPTH = 10

    def before_get(
        self, parser: Any, section: str, option: str, value: str, defaults: Any
    ) -> str:
        # Delegate interpolation to the Rust intrinsic via the parser's handle
        return str(
            _molt_configparser_interpolate_basic(
                parser._handle, str(section), value
            )
        )

    def before_set(self, parser: Any, section: str, option: str, value: str) -> str:
        # Validate the value for interpolation markers.
        tmp_value = value.replace("%%", "")
        tmp_value = tmp_value.replace("%(", "")
        if "%" in tmp_value:
            raise ValueError(
                "invalid interpolation syntax in %r at position %d"
                % (value, tmp_value.index("%"))
            )
        return value


class ExtendedInterpolation(Interpolation):
    """Advanced variant of interpolation, supports the syntax used by
    ``zc.buildout``. Enables cross-section references using the
    ``${section:option}`` syntax."""

    _MAX_INTERPOLATION_DEPTH = 10

    def before_get(
        self, parser: Any, section: str, option: str, value: str, defaults: Any
    ) -> str:
        # Delegate interpolation to the Rust intrinsic via the parser's handle
        return str(
            _molt_configparser_interpolate_extended(
                parser._handle, str(section), value
            )
        )

    def before_set(self, parser: Any, section: str, option: str, value: str) -> str:
        return value


class InterpolationError(Error):
    """Base class for interpolation-related exceptions."""

    def __init__(self, option: str, section: str, msg: str = "") -> None:
        self.option = option
        self.section = section
        super().__init__(msg)


class InterpolationMissingOptionError(InterpolationError):
    """A string substitution required a setting which was not available."""

    def __init__(self, option: str, section: str, rawval: str, reference: str) -> None:
        self.reference = reference
        msg = (
            f"Bad value substitution: option {option!r} in section "
            f"{section!r} contains an interpolation key {reference!r} "
            f"which is not a valid option name. Raw value: {rawval!r}"
        )
        super().__init__(option, section, msg)


class InterpolationSyntaxError(InterpolationError):
    """Raised when the source text contains invalid interpolation syntax."""


class InterpolationDepthError(InterpolationError):
    """Raised when substitutions are nested too deeply."""

    def __init__(self, option: str, section: str, rawval: str) -> None:
        msg = (
            f"Recursion limit exceeded in value substitution: option "
            f"{option!r} in section {section!r} contains an interpolation "
            f"key which cannot be substituted in {self._MAX_INTERPOLATION_DEPTH} "
            f"steps. Raw value: {rawval!r}"
        )
        super().__init__(option, section, msg)

    _MAX_INTERPOLATION_DEPTH = 10


# --- ConfigParser ---

_UNSET = object()


class RawConfigParser:
    """ConfigParser that does not do interpolation."""

    def __init__(
        self,
        defaults: dict[str, str] | None = None,
        *,
        interpolation: Interpolation | None = _UNSET,  # type: ignore[assignment]
        **kwargs: Any,
    ) -> None:
        if interpolation is _UNSET:
            interpolation = None
        self._interpolation = interpolation
        self._handle = _molt_configparser_new(defaults, "none")

    def __del__(self) -> None:
        handle = getattr(self, "_handle", None)
        if handle is not None:
            try:
                _molt_configparser_drop(handle)
            except Exception:
                pass

    def read_string(self, string: str, source: str = "<string>") -> None:
        _molt_configparser_read_string(self._handle, str(string))

    def read(
        self, filenames: str | list[str], encoding: str | None = None
    ) -> list[str]:
        if isinstance(filenames, str):
            filenames = [filenames]
        result: list[str] = []
        for filename in filenames:
            read_files = _molt_configparser_read(self._handle, str(filename))
            if read_files:
                result.extend(read_files)
        return result

    def read_file(self, f: Any, source: str | None = None) -> None:
        content = f.read()
        src = source if source is not None else getattr(f, "name", "<???>")
        _molt_configparser_read_file(self._handle, str(content), str(src))

    def read_dict(self, dictionary: dict[str, dict[str, str]], source: str = "<dict>") -> None:
        for section, keys in dictionary.items():
            try:
                self.add_section(section)
            except (DuplicateSectionError, ValueError):
                pass
            for key, value in keys.items():
                self.set(section, key, value)

    def defaults(self) -> dict[str, str]:
        pairs = _molt_configparser_defaults(self._handle)
        return {k: v for k, v in pairs}

    def sections(self) -> list[str]:
        return list(_molt_configparser_sections(self._handle))

    def has_section(self, section: str) -> bool:
        return bool(_molt_configparser_has_section(self._handle, str(section)))

    def has_option(self, section: str, option: str) -> bool:
        return bool(
            _molt_configparser_has_option(self._handle, str(section), str(option))
        )

    def get(self, section: str, option: str, *, fallback: Any = _UNSET) -> str:
        if self.has_option(str(section), str(option)):
            return str(
                _molt_configparser_get(self._handle, str(section), str(option), None)
            )
        if fallback is not _UNSET:
            return fallback
        raise KeyError(option)

    def getint(self, section: str, option: str, *, fallback: Any = _UNSET) -> int:
        if self.has_option(str(section), str(option)):
            return int(
                _molt_configparser_getint(self._handle, str(section), str(option), None)
            )
        if fallback is not _UNSET:
            return fallback
        raise KeyError(option)

    def getfloat(self, section: str, option: str, *, fallback: Any = _UNSET) -> float:
        if self.has_option(str(section), str(option)):
            return float(
                _molt_configparser_getfloat(
                    self._handle, str(section), str(option), None
                )
            )
        if fallback is not _UNSET:
            return fallback
        raise KeyError(option)

    def getboolean(self, section: str, option: str, *, fallback: Any = _UNSET) -> bool:
        if self.has_option(str(section), str(option)):
            return bool(
                _molt_configparser_getboolean(
                    self._handle, str(section), str(option), None
                )
            )
        if fallback is not _UNSET:
            return fallback
        raise KeyError(option)

    def options(self, section: str) -> list[str]:
        return list(_molt_configparser_options(self._handle, str(section)))

    def items(
        self,
        section: str = _UNSET,
        raw: bool = False,
        vars: dict[str, str] | None = None,
    ) -> list[tuple[str, str]]:  # type: ignore[assignment]
        if section is _UNSET:
            # Return list of (section_name, section_proxy) -- simplified for Molt.
            result: list[tuple[str, str]] = []
            for s in self.sections():
                for key, val in _molt_configparser_items(self._handle, s):
                    result.append((key, val))
            return result
        return list(_molt_configparser_items(self._handle, str(section)))

    def set(self, section: str, option: str, value: str | None = None) -> None:
        _molt_configparser_set(self._handle, str(section), str(option), value)

    def add_section(self, section: str) -> None:
        _molt_configparser_add_section(self._handle, str(section))

    def remove_section(self, section: str) -> bool:
        return bool(_molt_configparser_remove_section(self._handle, str(section)))

    def remove_option(self, section: str, option: str) -> bool:
        return bool(
            _molt_configparser_remove_option(self._handle, str(section), str(option))
        )

    def write(self, fp: Any, space_around_delimiters: bool = True) -> None:
        content = str(_molt_configparser_write_string(self._handle))
        fp.write(content)


class ConfigParser(RawConfigParser):
    """ConfigParser with interpolation support (default: BasicInterpolation)."""

    def __init__(
        self,
        defaults: dict[str, str] | None = None,
        *,
        interpolation: Interpolation | None = _UNSET,  # type: ignore[assignment]
        **kwargs: Any,
    ) -> None:
        if interpolation is _UNSET:
            interpolation = BasicInterpolation()
        self._interpolation = interpolation
        interp_name = "basic"
        if isinstance(interpolation, ExtendedInterpolation):
            interp_name = "extended"
        elif interpolation is None:
            interp_name = "none"
        self._handle = _molt_configparser_new(defaults, interp_name)

    def get(
        self,
        section: str,
        option: str,
        *,
        raw: bool = False,
        fallback: Any = _UNSET,
        vars: dict[str, str] | None = None,
        **kwargs: Any,
    ) -> str:
        if not self.has_option(str(section), str(option)):
            if fallback is not _UNSET:
                return fallback
            raise KeyError(option)

        # Get the raw value from the Rust backend
        raw_value = _molt_configparser_get_raw(
            self._handle, str(section), str(option)
        )
        if raw_value is None:
            if fallback is not _UNSET:
                return fallback
            raise KeyError(option)
        value = str(raw_value)

        if raw or self._interpolation is None:
            return value

        # Delegate interpolation to the intrinsic-backed interpolation class
        defaults_dict: dict[str, str] = {}
        for k, v in _molt_configparser_items(self._handle, str(section)):
            defaults_dict[k] = v
        if vars is not None:
            defaults_dict.update(vars)
        return self._interpolation.before_get(
            self, str(section), str(option), value, defaults_dict
        )
