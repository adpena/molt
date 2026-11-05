"""Render sole attached extern entries next to their typed Rust body owners."""

from __future__ import annotations
from dataclasses import dataclass
from pathlib import Path
from functools import lru_cache
import re
from molt.rust_source_scan import mask_rust_comments_and_strings, _rust_delimiter_ends

BODY_SUFFIX = "_attached_body"


@lru_cache(maxsize=1)
def exported_macro_symbols(root: Path) -> set[str]:
    """Literal link identities at invocations of actual C-export producers."""
    sources = [
        p.read_text(encoding="utf-8")
        for p in root.glob("runtime/*/src/**/*.rs")
        if "attached_exports_generated" not in p.parts
        and "test" not in p.stem
        and "tests" not in p.parts
    ]
    producers = set()
    for text in sources:
        code = mask_rust_comments_and_strings(text)
        ends = _rust_delimiter_ends(code)
        for match in re.finditer(r"\bmacro_rules!\s+(\w+)\s*\{", code):
            opening = match.end() - 1
            body = text[opening : ends[opening]]
            if (
                re.search(r'pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+\$', body)
                and "attached_symbol!" in body
            ):
                producers.add(match[1])
    symbols = set()
    for text in sources:
        code = mask_rust_comments_and_strings(text)
        ends = _rust_delimiter_ends(code)
        for match in re.finditer(r"\b(\w+)!\s*\(", code):
            if match[1] not in producers:
                continue
            opening = match.end() - 1
            symbols.update(
                re.findall(r'"((?:__)?molt_\w+)"', text[opening : ends[opening]])
            )
    return symbols


@dataclass(frozen=True)
class RustFunction:
    name: str
    start: int
    params_open: int
    params_end: int
    body_open: int
    end: int
    unsafe: bool
    external: bool
    attrs: tuple[str, ...]
    attrs_start: int
    params: str
    result: str


def functions(text: str, *, include_private: bool = False) -> list[RustFunction]:
    code = mask_rust_comments_and_strings(text)
    ends = _rust_delimiter_ends(code)
    reverse = {end - 1: start for start, end in ends.items()}
    visibility = (
        r"(?:\bpub(?:\([^)]*\))?\s+)?" if include_private else r"\bpub(?:\([^)]*\))?\s+"
    )
    pattern = re.compile(
        visibility
        + r"(?P<unsafe>unsafe\s+)?(?P<extern>extern\s+)?fn\s+(?P<name>[A-Za-z_]\w*)(?:\s*<[^{};]*?>)?\s*\("
    )
    out = []
    for m in pattern.finditer(code):
        opening = m.end() - 1
        pe = ends.get(opening)
        if pe is None:
            continue
        bo = code.find("{", pe)
        semi = code.find(";", pe)
        if bo < 0 or 0 <= semi < bo:
            continue
        end = ends.get(bo)
        if end is None:
            continue
        attrs = []
        cursor = m.start()
        while True:
            before = cursor - 1
            while before >= 0 and code[before].isspace():
                before -= 1
            if before < 0 or code[before] != "]" or before not in reverse:
                break
            ao = reverse[before]
            hash_pos = ao - 1
            while hash_pos >= 0 and code[hash_pos].isspace():
                hash_pos -= 1
            if hash_pos < 0 or code[hash_pos] != "#":
                break
            attrs.insert(0, text[hash_pos : before + 1])
            cursor = hash_pos
        result = text[pe:bo].strip()
        if result.startswith("->"):
            result = result[2:].strip()
        else:
            result = "()"
        out.append(
            RustFunction(
                m["name"],
                m.start(),
                opening,
                pe,
                bo,
                end,
                bool(m["unsafe"]),
                bool(m["extern"]),
                tuple(attrs),
                cursor,
                text[opening + 1 : pe - 1],
                result,
            )
        )
    return out


def split_params(text: str) -> list[str]:
    from tools.wasm_abi_gen.manifest import _split_rust_top_level_commas
    from molt.rust_source_scan import rust_comment_segments

    for _line, comment in reversed(rust_comment_segments(text)):
        offset = text.rfind(comment)
        assert offset >= 0
        text = text[:offset] + " " * len(comment) + text[offset + len(comment) :]
    return _split_rust_top_level_commas(text)


def typed_bodies(root: Path):
    for path in sorted(root.glob("runtime/*/src/**/*.rs")):
        if (
            "attached_exports_generated" in path.parts
            or path.name.endswith("tests.rs")
            or "tests" in path.parts
        ):
            continue
        text = path.read_text(encoding="utf-8")
        for f in functions(text):
            if f.name.endswith(BODY_SUFFIX):
                yield path, text, f


def render_attached_entries(data: dict, root: Path) -> dict[Path, str]:
    from tools.wasm_abi_gen.execution_context import context_suffix

    public = set(
        re.findall(
            r"\b(molt_\w+)\s*\(",
            (root / "include/molt/molt.h").read_text(encoding="utf-8"),
        )
    )
    outputs = {}
    grouped = {}
    for path, text, f in typed_bodies(root):
        grouped.setdefault(path, []).append(f)
    for path, items in grouped.items():
        crate_root = next(p for p in path.parents if (p / "Cargo.toml").is_file())
        relative = path.relative_to(crate_root / "src")
        output = crate_root / "src/attached_exports_generated" / relative
        lines = ["// @generated by tools/gen_wasm_abi.py. DO NOT EDIT.\n"]
        for f in items:
            name = f.name.removesuffix(BODY_SUFFIX)
            params = split_params(f.params)
            assert params and "PyToken" in params[0], (path, f.name, f.params)
            token_type = params[0].split(":", 1)[1].strip()
            args = [
                p.split(":", 1)[0].strip().removeprefix("mut ").strip()
                for p in params[1:]
            ]
            assert all(re.fullmatch(r"\w+", a) for a in args), (path, name, args)
            cfg = [
                a
                for a in f.attrs
                if re.match(r"#\[cfg\s*\(", a) or a == "#[allow(non_snake_case)]"
            ]
            attrs = "\n".join(cfg)
            if attrs:
                attrs += "\n"
            signature = ", ".join(["__context: u64", *params[1:]])
            result = "" if f.result == "()" else " -> " + f.result
            call = f"{f.name}(" + ", ".join(["_py", *args]) + ")"
            if f.unsafe:
                call = "unsafe { " + call + " }"
            lines += [
                attrs,
                f'#[unsafe(export_name = "{name}{context_suffix(data)}")]\n',
                "#[allow(clippy::missing_safety_doc)]\n",
                f'pub unsafe extern "C" fn {name}({signature}){result} {{\n',
                f"    let _py: {token_type} = unsafe {{ &*std::ptr::with_exposed_provenance(__context as usize) }};\n",
                f"    {call}\n}}\n",
            ]
            if name in public and crate_root.name == "molt-runtime":
                # This is the genuine public C admission protocol, sharing the
                # sole typed implementation with the compiled attached entry.
                lines += [
                    attrs,
                    f'#[unsafe(export_name = "{name}")]\n',
                    "#[allow(clippy::missing_safety_doc)]\n",
                    f'pub unsafe extern "C" fn {name}_public_c_entry('
                    + ", ".join(params[1:])
                    + f"){result} {{\n",
                    "    let guard = crate::concurrency::RuntimeExecutionGuard::enter();\n",
                    "    let _py = guard.token();\n",
                    f"    {call}\n}}\n",
                ]
        outputs[output] = "".join(lines)
    return outputs
