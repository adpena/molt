"""Location-neutral source-extension build and wheel metadata authority."""

from __future__ import annotations

import hashlib
import ast
import configparser
import json
import os
import re
import tokenize
from io import StringIO
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePath
from typing import Any
from molt.cli.compiler_target import (
    SourceExtensionCompilerDialect,
    source_extension_compiler_dialect,
)
from molt.cli.source_extension_target import (
    SourceExtensionLinkDialect,
    source_extension_link_dialect,
)
from molt.llvm_linker_roles import (
    executable_entrypoint_name,
    executable_selects_linker_role,
)
from molt.cli.source_extension_manifest_codec import (
    _expand_source_extension_manifest_authorities,
)

from molt.cli.source_extension_object_closure import (
    finalize_source_extension_object_closure,
)

_MESON_TRANSIENT_DEPENDENCY_ID_RE = re.compile(r"dep[0-9]+")
_URL_SCHEME_RE = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*://")
_FILE_URL_RE = re.compile(r"(?i)file://")
_WINDOWS_ABSOLUTE_RE = re.compile(r"(?i)(?<![A-Za-z0-9_])([A-Z]):/{1,}")
_UNC_ABSOLUTE_RE = re.compile(r"(?<!:)//[^/\s]+/[^/\s]+")
_POSIX_ABSOLUTE_RE = re.compile(
    r"(?:^|(?<=[=,:;\s'\"\(\[\{]))/(?!/)[^/\s'\"\)\]\}]+(?:/[^\s'\"\)\]\}]*)?"
)
_HOME_PATH_RE = re.compile(
    r"(?i)(?:^|(?<=[=,:;\s'\"\(\[\{]))(?:~(?:/|$)|\$HOME(?:/|$)|"
    r"\$\{HOME\}(?:/|$)|%(?:USERPROFILE|HOME)%(?:/|$))"
)
_JOINED_PATH_PREFIXES = (
    "-I",
    "-L",
    "-isystem",
    "-iquote",
    "-include",
    "--sysroot=",
    "/I",
    "/FI",
    "/Fa",
    "/Fo",
    "/Fd",
    "/Fe",
    "/Fi",
    "/Fp",
    "/FR",
    "/FU",
    "/DEF:",
    "/IMPLIB:",
    "/MANIFESTFILE:",
    "/OUT:",
    "/PDB:",
    "/PDBALTPATH:",
    "/PGD:",
    "-MF",
    "/LIBPATH:",
    "@",
)
_JOINED_PATH_FLAG_RE = re.compile(
    r"(?i)(?:^|(?<=\s))(?:"
    + "|".join(
        re.escape(prefix)
        for prefix in sorted(_JOINED_PATH_PREFIXES, key=len, reverse=True)
    )
    + r")(?P<path>(?:[A-Z]:/+|//|/)[^\s'\"]+)"
)
_MSVC_PATH_FLAG_PREFIXES = tuple(
    sorted(
        (prefix for prefix in _JOINED_PATH_PREFIXES if prefix.startswith("/")),
        key=len,
        reverse=True,
    )
)
# Option syntax is not driver selection. Only a schema-owned command or its
# explicitly associated option list may interpret this syntax as an option.
_MSVC_OPTION_RE = re.compile(r"/[A-Za-z?][A-Za-z0-9?+_.-]*(?::[^/\\\s]+)?")
_COMMAND_FIELDS = frozenset(
    {
        "compiler",
        "linker",
        "arguments",
        "command",
        "compile_command",
        "symbol_command",
        "commands",
        "tool_commands",
    }
)
_COMMAND_ROLES = frozenset(
    {"c", "cpp", "cc", "cxx", "ar", "ranlib", "ld", "wasm_ld", "nm", "strip"}
)
_OPTION_FIELDS = frozenset(
    {
        "parameters",
        "compile_args",
        "extra_compile_args",
        "link_args",
    }
)


def _recorded_command_argv(value: Any) -> tuple[str, ...]:
    if isinstance(value, str):
        # Compile database strings use the same Windows quoting grammar at
        # ingestion and at location-neutrality validation.
        from molt.cli.source_extensions import _split_windows_command_line

        return tuple(_split_windows_command_line(value) or ())
    if (
        isinstance(value, Sequence)
        and not isinstance(value, (str, bytes))
        and all(isinstance(item, str) for item in value)
    ):
        return tuple(value)
    return ()


def _command_uses_msvc_options(value: Any) -> bool:
    argv = _recorded_command_argv(value)
    if not argv:
        return False
    executable = Path(argv[0])
    if executable_selects_linker_role(
        executable, "lld-link"
    ) or executable_entrypoint_name(executable) in {"link", "lib", "llvm-lib"}:
        return True
    try:
        return (
            source_extension_compiler_dialect(argv)
            is SourceExtensionCompilerDialect.CLANG_CL
        )
    except ValueError:
        return False


def _mapping_uses_msvc_options(value: Mapping[object, Any]) -> bool:
    if any(
        _command_uses_msvc_options(value.get(field))
        for field in ("compiler", "linker", "compile_command")
    ):
        return True
    units = value.get("compile_units")
    return (
        isinstance(units, Sequence)
        and not isinstance(units, (str, bytes))
        and bool(units)
        and all(
            isinstance(unit, Mapping)
            and _command_uses_msvc_options(unit.get("compiler"))
            for unit in units
        )
    )


def _neutral_msvc_option(token: str, *, canonical_path_only: bool = False) -> bool:
    for prefix in _MSVC_PATH_FLAG_PREFIXES:
        if token.upper().startswith(prefix.upper()):
            payload = token[len(prefix) :]
            if canonical_path_only and not payload.startswith("@"):
                return False
            return not _residual_producer_paths(payload)
    if canonical_path_only:
        return False
    if token.startswith("/clang:"):
        return not _residual_producer_paths(token.removeprefix("/clang:"))
    if token.startswith(("/D", "/U")):
        return not _residual_producer_paths(token[2:])
    return _MSVC_OPTION_RE.fullmatch(token) is not None


def _inside_url_token(value: str, index: int) -> bool:
    token_start = index
    while token_start and value[token_start - 1] not in " \t\r\n'\"()[]{}":
        token_start -= 1
    match = _URL_SCHEME_RE.search(value[token_start:index])
    return match is not None and match.group(0).casefold() != "file://"


def _root_occurrence_is_path(value: str, index: int) -> bool:
    if index == 0 or value[index - 1] in "=,:; \t\r\n'\"()[]{}":
        return True
    prefix = value[max(0, index - 16) : index]
    return any(prefix.endswith(flag) for flag in _JOINED_PATH_PREFIXES)


def _filesystem_root_pattern(root: PurePath) -> re.Pattern[str]:
    rendered = root.as_posix().rstrip("/")
    if not rendered:
        raise ValueError("filesystem root cannot be a location identity authority")
    components = rendered.split("/")
    prefix = ""
    if len(components) >= 2 and components[0].endswith(":") and not components[1]:
        components.pop(1)
    if rendered.startswith("//"):
        while components and not components[0]:
            components.pop(0)
        prefix = r"(?<!:)//+"
    # A root may be spelled with forward slashes, single backslashes (raw
    # text) or escaped backslashes (a JSON or Python string literal); every
    # spelling names the same directory.
    body = r"(?:/+|\\+)".join(re.escape(component) for component in components)
    return re.compile(
        prefix + body + r"""(?:(?P<separator>/+|\\+)|(?=$|[=;,\s'"\)\]\}]))""",
        re.IGNORECASE if root.drive else 0,
    )


def _residual_producer_paths(value: Any, *, location: str = "$") -> list[str]:
    findings: list[str] = []

    def inspect(text: str, item_location: str, *, msvc_options: bool = False) -> None:
        normalized = text.replace("\\", "/")
        if _FILE_URL_RE.search(normalized):
            findings.append(f"{item_location}: residual file URL in {text!r}")
            return
        if _HOME_PATH_RE.search(normalized):
            findings.append(
                f"{item_location}: residual home-relative producer path in {text!r}"
            )
            return
        if msvc_options and _neutral_msvc_option(normalized):
            return
        for pattern, kind in (
            (_WINDOWS_ABSOLUTE_RE, "drive"),
            (_UNC_ABSOLUTE_RE, "UNC"),
            (_POSIX_ABSOLUTE_RE, "POSIX"),
        ):
            for match in pattern.finditer(normalized):
                if kind == "POSIX" and _neutral_msvc_option(
                    match.group(0), canonical_path_only=True
                ):
                    continue
                if not _inside_url_token(normalized, match.start()):
                    findings.append(
                        f"{item_location}: residual {kind} producer path in {text!r}"
                    )
                    return
        for match in _JOINED_PATH_FLAG_RE.finditer(normalized):
            if not _inside_url_token(normalized, match.start("path")):
                findings.append(
                    f"{item_location}: residual joined-flag producer path in {text!r}"
                )
                return

    def walk(
        item: Any,
        item_location: str,
        *,
        msvc_options: bool = False,
        command_field: bool = False,
        source_plan_msvc_linker: bool = False,
    ) -> None:
        if isinstance(item, str):
            if command_field and _command_uses_msvc_options(item):
                for index, argument in enumerate(_recorded_command_argv(item)):
                    inspect(
                        argument,
                        f"{item_location}.argv[{index}]",
                        msvc_options=index > 0,
                    )
            else:
                inspect(item, item_location, msvc_options=msvc_options)
        elif isinstance(item, Sequence) and not isinstance(item, (str, bytes)):
            is_command = command_field and _command_uses_msvc_options(item)
            for index, child in enumerate(item):
                walk(
                    child,
                    f"{item_location}[{index}]",
                    msvc_options=msvc_options or (is_command and index > 0),
                    command_field=command_field and not is_command,
                    source_plan_msvc_linker=source_plan_msvc_linker,
                )
        elif isinstance(item, Mapping):
            owner_uses_msvc = _mapping_uses_msvc_options(item)
            for raw_key, child in item.items():
                key = str(raw_key)
                inspect(key, f"{item_location}.<key>")
                child_is_msvc_plan = False
                if (
                    key == "source_plan"
                    and isinstance(child, Mapping)
                    and child.get("kind") == "meson-intro-targets"
                    and isinstance(item.get("target_triple"), str)
                ):
                    child_is_msvc_plan = (
                        source_extension_link_dialect(item["target_triple"])
                        is SourceExtensionLinkDialect.COFF_MSVC
                    )
                walk(
                    child,
                    f"{item_location}.{key}",
                    msvc_options=(key in _OPTION_FIELDS and owner_uses_msvc)
                    or (source_plan_msvc_linker and key == "arguments"),
                    source_plan_msvc_linker=child_is_msvc_plan
                    or source_plan_msvc_linker,
                    command_field=key in _COMMAND_FIELDS
                    or key == "compile_commands"
                    or (command_field and key in _COMMAND_ROLES),
                )

    walk(value, location, command_field=isinstance(value, (list, tuple)))
    return findings


def _require_location_neutral(value: Any, *, authority: str) -> None:
    semantic_value = (
        _expand_source_extension_manifest_authorities(value)
        if isinstance(value, Mapping) and "build_authorities" in value
        else value
    )
    findings = _residual_producer_paths(semantic_value)
    if findings:
        preview = "; ".join(findings[:8])
        suffix = "" if len(findings) <= 8 else f"; +{len(findings) - 8} more"
        raise ValueError(
            f"{authority} retains producer filesystem paths: {preview}{suffix}"
        )


def require_source_extension_machine_file_location_neutral(
    text: str,
    *,
    authority: str,
) -> None:
    """Validate generated Meson machine text with explicit option ownership.

    The staging caller selects this grammar by machine-file artifact kind; raw
    source text is never promoted to compiler context by mentioning a driver.
    Only literal binary argv and their C/C++ built-in option arrays are options.
    Other sections and properties remain ordinary path-checked text.
    """
    for line in text.splitlines():
        if line.lstrip().startswith(("#", ";")):
            _require_location_neutral(line, authority=authority)

    def literal(raw: str, field: str) -> Any:
        try:
            # literal_eval discards trailing Python comments. They remain in
            # the staged text, so validate them as text, never option context.
            for token in tokenize.generate_tokens(StringIO(raw).readline):
                if token.type == tokenize.COMMENT:
                    _require_location_neutral(token.string, authority=authority)
            return ast.literal_eval(raw)
        except (ValueError, SyntaxError, tokenize.TokenError) as exc:
            raise ValueError(
                f"{authority} {field} must be literal argv: {exc}"
            ) from exc

    parser = configparser.ConfigParser(
        interpolation=None, delimiters=("=",), strict=True
    )
    try:
        parser.read_string(text)
    except configparser.Error as exc:
        raise ValueError(
            f"{authority} has invalid Meson machine-file syntax: {exc}"
        ) from exc
    if parser.defaults():
        raise ValueError(f"{authority} cannot use implicit machine-file defaults")
    binaries: dict[str, tuple[str, ...]] = {}
    for section in parser.sections():
        _require_location_neutral(section, authority=authority)
        for name, raw in parser.items(section):
            _require_location_neutral(name, authority=authority)
            if section != "binaries":
                continue
            value = literal(raw, f"binary {name}")
            argv = (value,) if isinstance(value, str) else value
            if (
                not isinstance(argv, (list, tuple))
                or not argv
                or any(not isinstance(item, str) or not item for item in argv)
            ):
                raise ValueError(
                    f"{authority} binary {name} must be non-empty string argv"
                )
            binaries[name] = tuple(argv)
            _require_location_neutral({"command": argv}, authority=authority)
    for section in parser.sections():
        if section == "binaries":
            continue
        for name, raw in parser.items(section):
            role = name.split("_", 1)[0]
            if (
                section == "built-in options"
                and name in {"c_args", "cpp_args", "c_link_args", "cpp_link_args"}
                and role in binaries
            ):
                args = literal(raw, f"option {name}")
                if not isinstance(args, (list, tuple)) or any(
                    not isinstance(arg, str) for arg in args
                ):
                    raise ValueError(f"{authority} option {name} must be string argv")
                _require_location_neutral(
                    {"compiler": binaries[role], "parameters": args},
                    authority=authority,
                )
            else:
                _require_location_neutral(raw, authority=authority)


def _ordered_location_roots(
    roots: Sequence[tuple[PurePath | None, str]],
) -> tuple[tuple[PurePath, str], ...]:
    """The location roots in their declared, canonical order.

    The declared order is the neutralization order: where two roots contain
    the same path the earlier one wins, so a caller declares every root that
    can sit inside another before that container. The order is therefore one
    function of the roles and never of the host layout: the same roots yield
    the same path-map arguments and the same canonical metadata on every
    machine. A layout that nests a root inside an earlier-declared one is
    refused rather than reordered, because reordering would make the recorded
    command depend on where this host happened to put its directories.

    A producer location may appear in build metadata under more than one
    spelling of the same directory: the lexical path the tool was handed (an
    installer's version alias junction, a relative segment) and the resolved
    real path. Both spellings map to the same token. A virtual root (a
    PurePosixPath such as the Meson install prefix) names no directory on this
    machine and is matched exactly as spelled.
    """
    deduped: list[tuple[PurePath, str]] = []
    seen: set[str] = set()
    for path, replacement in roots:
        if path is None:
            continue
        candidates = (
            (Path(os.path.abspath(path)), path.resolve())
            if isinstance(path, Path)
            else (path,)
        )
        for candidate in candidates:
            key = os.path.normcase(os.fspath(candidate))
            if key not in seen:
                seen.add(key)
                deduped.append((candidate, replacement))
    for later, (candidate, replacement) in enumerate(deduped):
        for earlier, earlier_replacement in deduped[:later]:
            if candidate != earlier and candidate.is_relative_to(earlier):
                raise ValueError(
                    "location roots are not in canonical order: "
                    f"{replacement} ({candidate}) lies inside the earlier "
                    f"{earlier_replacement} ({earlier}); declare the nested "
                    "root before its container"
                )
    return tuple(deduped)


def _source_extension_deterministic_path_args(
    *,
    compiler_command: Sequence[str],
    roots: Sequence[tuple[PurePath | None, str]],
) -> list[str]:
    if not compiler_command:
        return []
    ordered = _ordered_location_roots(roots)
    dialect = source_extension_compiler_dialect(compiler_command)
    return [
        dialect.forward(argument)
        for path, replacement in ordered
        for argument in (
            f"-ffile-prefix-map={path}={replacement}",
            f"-fdebug-prefix-map={path}={replacement}",
            f"-fmacro-prefix-map={path}={replacement}",
        )
    ]


def _canonicalize_location_string(
    value: str, location_roots: Sequence[tuple[PurePath | None, str]]
) -> str:
    return _canonicalize_location_string_ordered(
        value, _ordered_location_roots(location_roots)
    )


_PATH_SPAN_DELIMITERS = frozenset(" \t\r\n'\"()[]{};,<>|")
_SEPARATOR_STYLE_SLASH = "/"
_SEPARATOR_STYLE_RAW = "raw-backslash"
_SEPARATOR_STYLE_ESCAPED = "escaped-backslash"


def _path_span_end(text: str, start: int, separator_style: str) -> tuple[str, int]:
    """Read the path that continues after a matched root.

    Returns the continuation with its separators spelled ``/`` and the index
    where the span ends. Separators keep the style the root was spelled in:
    a forward slash, a raw backslash, or an escaped backslash pair (a JSON or
    Python string literal), so a lone backslash inside an escaped-style span
    starts an escape sequence and ends the path rather than joining it.
    """
    tail: list[str] = []
    index = start
    length = len(text)
    while index < length:
        char = text[index]
        if char in _PATH_SPAN_DELIMITERS:
            break
        if char == "/":
            tail.append("/")
            while index < length and text[index] == "/":
                index += 1
            continue
        if char == "\\":
            run_end = index
            while run_end < length and text[run_end] == "\\":
                run_end += 1
            run = run_end - index
            if separator_style == _SEPARATOR_STYLE_SLASH or (
                separator_style == _SEPARATOR_STYLE_ESCAPED and run % 2
            ):
                break
            tail.append("/")
            index = run_end
            continue
        tail.append(char)
        index += 1
    return "".join(tail), index


def _separator_style(separator: str) -> str:
    if "/" in separator:
        return _SEPARATOR_STYLE_SLASH
    return _SEPARATOR_STYLE_ESCAPED if len(separator) >= 2 else _SEPARATOR_STYLE_RAW


def _canonicalize_location_string_ordered(
    value: str, ordered_roots: Sequence[tuple[PurePath, str]]
) -> str:
    """Rewrite every path span rooted at a location root to its token.

    Only the root and the path continuing from it change; the rest of the
    text (escape sequences in a Python or JSON literal, compile flags) is
    preserved byte for byte, so canonicalizing an installed Python source
    never alters its meaning.
    """
    canonical = value
    for root, token in ordered_roots:
        pattern = _filesystem_root_pattern(root)
        pieces: list[str] = []
        position = 0
        for match in pattern.finditer(canonical):
            if match.start() < position:
                continue
            if _inside_url_token(
                canonical, match.start()
            ) or not _root_occurrence_is_path(canonical, match.start()):
                continue
            pieces.append(canonical[position : match.start()])
            separator = match.group("separator")
            if not separator:
                pieces.append(token)
                position = match.end()
                continue
            tail, position = _path_span_end(
                canonical, match.end(), _separator_style(separator)
            )
            pieces.append(f"{token}/{tail}")
        pieces.append(canonical[position:])
        canonical = "".join(pieces)
    return canonical


def _canonicalize_locations(
    value: Any,
    location_roots: Sequence[tuple[PurePath | None, str]],
    source_paths: Mapping[Path, str] | None = None,
) -> Any:
    ordered_roots = _ordered_location_roots(location_roots)
    resolved_sources: dict[str, str] = {}
    for path, replacement in (source_paths or {}).items():
        for candidate in (path.expanduser(), path.resolve()):
            key = os.path.normcase(os.path.normpath(os.fspath(candidate)))
            prior = resolved_sources.setdefault(key, replacement)
            if prior != replacement:
                raise ValueError(
                    "source-path canonicalization has conflicting identities"
                )

    def canonicalize(item: Any) -> Any:
        if isinstance(item, str):
            expanded = os.path.expanduser(item)
            candidate = (
                os.path.normcase(os.path.normpath(expanded))
                if source_paths is not None and os.path.isabs(expanded)
                else None
            )
            if candidate is not None and candidate in resolved_sources:
                return resolved_sources[candidate]
            return _canonicalize_location_string_ordered(item, ordered_roots)
        if isinstance(item, list):
            return [canonicalize(child) for child in item]
        if isinstance(item, dict):
            canonical: dict[str, Any] = {}
            for raw_key, child in item.items():
                key = _canonicalize_location_string_ordered(str(raw_key), ordered_roots)
                if key in canonical:
                    raise ValueError(
                        "location canonicalization collapses distinct metadata keys: "
                        f"{raw_key!r} -> {key!r}"
                    )
                canonical[key] = canonicalize(child)
            return canonical
        return item

    return canonicalize(value)


def _canonicalize_meson_metadata(
    value: Any, location_roots: Sequence[tuple[PurePath | None, str]]
) -> Any:
    canonical = _canonicalize_locations(value, location_roots)
    dependency_ids: dict[str, str] = {}

    def replace(item: Any, *, in_dependencies: bool = False) -> Any:
        if isinstance(item, str):
            if not in_dependencies or not _MESON_TRANSIENT_DEPENDENCY_ID_RE.fullmatch(
                item
            ):
                return item
            return dependency_ids.setdefault(
                item, f"@meson-dependency/{len(dependency_ids):04d}"
            )
        if isinstance(item, list):
            return [replace(child, in_dependencies=in_dependencies) for child in item]
        if isinstance(item, dict):
            return {
                key: replace(item[key], in_dependencies=key == "dependencies")
                for key in sorted(item)
            }
        return item

    return replace(canonical)


def _canonical_json_sha256(
    path: Path,
    *,
    location_roots: Sequence[tuple[PurePath | None, str]],
    normalize_meson_dependency_ids: bool,
) -> str:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(
            f"cannot canonicalize JSON identity input {path}: {exc}"
        ) from exc
    canonical = (
        _canonicalize_meson_metadata(payload, location_roots)
        if normalize_meson_dependency_ids
        else _canonicalize_locations(payload, location_roots)
    )
    return hashlib.sha256(
        json.dumps(canonical, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    ).hexdigest()


def _canonical_extension_manifest_for_wheel(
    manifest: Mapping[str, Any],
    *,
    location_roots: Sequence[tuple[PurePath | None, str]],
    meson_plan_path: Path | None = None,
    compile_commands_path: Path | None = None,
) -> dict[str, Any]:
    canonical = _canonicalize_locations(dict(manifest), location_roots)
    assert isinstance(canonical, dict)
    source_plan = canonical.get("source_plan")
    if isinstance(source_plan, dict):
        if meson_plan_path is not None:
            source_plan["plan_sha256"] = _canonical_json_sha256(
                meson_plan_path,
                location_roots=location_roots,
                normalize_meson_dependency_ids=True,
            )
        if compile_commands_path is not None:
            source_plan["compile_commands_sha256"] = _canonical_json_sha256(
                compile_commands_path,
                location_roots=location_roots,
                normalize_meson_dependency_ids=False,
            )
        identity = dict(source_plan)
        identity.pop("digest", None)
        source_plan["digest"] = hashlib.sha256(
            json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if isinstance(canonical.get("build"), dict):
            canonical["build"]["source_plan_digest"] = source_plan["digest"]
    if isinstance(canonical.get("object_closure"), dict):
        finalize_source_extension_object_closure(canonical)
    return canonical
