"""Generate target token metadata from receipted CPython source, without Python execution.

CPython's Grammar/Tokens and Tools/build/generate_token.py define numbering.
Lib/token.py is an independent checked projection, including declaration and
operator order. Pending source pins do not enter the shipped target catalog.
"""

from __future__ import annotations

import ast
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

_ROOT = Path(__file__).resolve().parents[1]
if str(_ROOT) not in sys.path:
    sys.path.insert(0, str(_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.target_python import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS  # noqa: E402
from tools import gen_stdlib_module_union as sources  # noqa: E402
from tools.generator_io import generator_main, rustfmt_source  # noqa: E402

OUTPUT = _ROOT / "runtime/molt-stdlib-text/src/token_metadata_generated.rs"
_NAME = re.compile(r"[A-Z][A-Z0-9_]*")


@dataclass(frozen=True)
class TokenMetadata:
    python: str
    revision: str
    constants: tuple[tuple[str, int], ...]
    exact_types: tuple[tuple[str, str], ...]
    base_exports: tuple[str, ...]


def _assignments(text: str) -> dict[str, ast.expr]:
    result: dict[str, ast.expr] = {}
    for node in ast.parse(text).body:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    if target.id in result:
                        raise ValueError(f"duplicate CPython assignment: {target.id}")
                    result[target.id] = node.value
    return result


def derive_metadata(snapshot: sources.SourceSnapshot) -> TokenMetadata:
    evidence = {item.role: item for item in snapshot.inputs}
    grammar = evidence["token_grammar"].text
    module = _assignments(evidence["token_module"].text)
    generator = _assignments(evidence["token_generator"].text)
    constants: list[tuple[str, int]] = []
    operators: dict[str, str] = {}
    names: set[str] = set()
    for line in grammar.splitlines():
        fields = line.partition("#")[0].split()
        if not fields:
            continue
        if len(fields) > 2 or _NAME.fullmatch(fields[0]) is None:
            raise ValueError(f"invalid CPython token definition: {line!r}")
        name = fields[0]
        if name in names:
            raise ValueError(f"duplicate CPython token name: {name}")
        names.add(name)
        constants.append((name, len(constants)))
        if len(fields) == 2:
            symbol = ast.literal_eval(fields[1])
            if not isinstance(symbol, str) or not symbol or symbol in operators:
                raise ValueError(f"invalid or duplicate CPython operator: {line!r}")
            operators[symbol] = name
    if not constants:
        raise ValueError("CPython token grammar is empty")
    offset = ast.literal_eval(generator["NT_OFFSET"])
    if type(offset) is not int or offset <= len(constants) or offset > 32767:
        raise ValueError("CPython token nonterminal offset is not representable")
    constants.extend((("N_TOKENS", len(constants)), ("NT_OFFSET", offset)))
    observed = tuple(
        (name, value.value)
        for name, value in module.items()
        if _NAME.fullmatch(name) is not None
        and isinstance(value, ast.Constant)
        and type(value.value) is int
    )
    if tuple(constants) != observed:
        raise ValueError(
            f"CPython {snapshot.python}: Grammar/Tokens and Lib/token.py disagree"
        )
    exact = module["EXACT_TOKEN_TYPES"]
    if not isinstance(exact, ast.Dict):
        raise ValueError("CPython EXACT_TOKEN_TYPES must be a dictionary declaration")
    observed_operators = []
    for key, value in zip(exact.keys, exact.values):
        if not isinstance(key, ast.Constant) or not isinstance(value, ast.Name):
            raise ValueError("CPython exact token type must name a declared constant")
        observed_operators.append((key.value, value.id))
    expected_operators = tuple(sorted(operators.items()))
    if tuple(observed_operators) != expected_operators:
        raise ValueError(
            f"CPython {snapshot.python}: exact operator projection disagrees"
        )
    exports = ast.literal_eval(module["__all__"])
    if (
        not isinstance(exports, list)
        or not exports
        or not all(isinstance(x, str) for x in exports)
    ):
        raise ValueError("invalid CPython token base exports")
    return TokenMetadata(
        snapshot.python,
        snapshot.revision,
        tuple(constants),
        expected_operators,
        tuple(exports),
    )


def _variant(name: str) -> str:
    return "".join(part.title() for part in name.split("_"))


def render(tables: tuple[TokenMetadata, ...]) -> str:
    names = tuple(
        dict.fromkeys(name for table in tables for name, _ in table.constants)
    )
    if len(names) > 256 or len({_variant(name) for name in names}) != len(names):
        raise ValueError("CPython token identities cannot use the closed u8 domain")
    shared_operators = tuple(dict.fromkeys(table.exact_types for table in tables))
    shared_exports = tuple(dict.fromkeys(table.base_exports for table in tables))
    lines = [
        "// @generated by tools/gen_token_metadata.py; do not edit.\n",
        "// Source: pinned CPython Grammar/Tokens, Lib/token.py and Tools/build/generate_token.py.\n",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n#[repr(u8)]\n",
        "pub enum TokenConstant {\n",
        *(f"    {_variant(name)},\n" for name in names),
        "}\nimpl TokenConstant {\npub const fn name(self) -> &'static str {\nmatch self {\n",
        *(f"Self::{_variant(name)} => {json.dumps(name)},\n" for name in names),
        "}\n}\n}\n",
        "pub struct TokenTable {\ncodes: [i16; " + str(len(names)) + "],\n",
        "order: &'static [TokenConstant],\nexact: &'static [(&'static str, TokenConstant)],\n",
        "base_exports: &'static [&'static str],\n}\n",
        "impl TokenTable {\npub fn number(&self, token: TokenConstant) -> Option<i64> {\n",
        "let code = self.codes[token as usize];\n(code >= 0).then_some(i64::from(code))\n}\n",
        "pub fn constants(&self) -> impl Iterator<Item = (&'static str, i64)> + '_ {\n",
        "self.order.iter().map(|&token| (token.name(), i64::from(self.codes[token as usize])))\n}\n",
        "pub fn exact_types(&self) -> impl Iterator<Item = (&'static str, i64)> + '_ {\n",
        "self.exact.iter().map(|&(symbol, token)| (symbol, i64::from(self.codes[token as usize])))\n}\n",
        "pub fn base_exports(&self) -> &'static [&'static str] { self.base_exports }\n",
        "pub fn exports(&self) -> impl Iterator<Item = &'static str> + '_ {\n",
        "self.base_exports.iter().copied().chain(self.order.iter().map(|&token| token.name()))\n}\n}\n",
    ]
    for index, operators in enumerate(shared_operators):
        lines.append(f"const EXACT_{index}: &[(&str, TokenConstant)] = &[\n")
        lines.extend(
            f"({json.dumps(symbol)}, TokenConstant::{_variant(name)}),\n"
            for symbol, name in operators
        )
        lines.append("];\n")
    for index, exports in enumerate(shared_exports):
        lines.append(f"const EXPORTS_{index}: &[&str] = &[")
        lines.append(",".join(json.dumps(name) for name in exports) + "];\n")
    for index, table in enumerate(tables):
        values = dict(table.constants)
        lines.append(f"// CPython {table.python}, commit {table.revision}.\n")
        lines.append(f"static TABLE_{index}: TokenTable = TokenTable {{\ncodes: [")
        lines.append(
            ",".join(str(values.get(name, -1)) for name in names) + "],\norder: &[\n"
        )
        lines.extend(
            f"TokenConstant::{_variant(name)},\n" for name, _ in table.constants
        )
        lines.append(f"],\nexact: EXACT_{shared_operators.index(table.exact_types)},\n")
        lines.append(
            f"base_exports: EXPORTS_{shared_exports.index(table.base_exports)},\n}};\n"
        )
    lines.append(
        "pub fn token_table(major: i64, minor: i64) -> Option<&'static TokenTable> {\nmatch (major, minor) {\n"
    )
    for index, table in enumerate(tables):
        major, minor = table.python.split(".")
        lines.append(f"({major}, {minor}) => Some(&TABLE_{index}),\n")
    lines.append("_ => None,\n}\n}\n")
    return "".join(lines)


def generated_outputs() -> dict[Path, str]:
    snapshots = sources.pinned_snapshots(SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)
    tables = tuple(
        derive_metadata(snapshots[version])
        for version in SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
    )
    return {OUTPUT: rustfmt_source(render(tables), label="target token metadata")}


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
