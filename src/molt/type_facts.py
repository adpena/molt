from __future__ import annotations

import ast
import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable, Literal

TrustLevel = Literal["advisory", "guarded", "trusted"]
TypeHintPolicy = Literal["ignore", "trust", "check"]


@dataclass
class Fact:
    type: str
    trust: TrustLevel


@dataclass
class FunctionFacts:
    params: dict[str, Fact] = field(default_factory=dict)
    locals: dict[str, Fact] = field(default_factory=dict)
    returns: Fact | None = None


@dataclass
class ModuleFacts:
    globals: dict[str, Fact] = field(default_factory=dict)
    functions: dict[str, FunctionFacts] = field(default_factory=dict)


@dataclass
class TypeFacts:
    schema_version: int = 1
    created_at: str = field(
        default_factory=lambda: datetime.now(timezone.utc).isoformat()
    )
    tool: str = "molt-check"
    strict: bool = False
    modules: dict[str, ModuleFacts] = field(default_factory=dict)

    def module(self, name: str) -> ModuleFacts:
        if name not in self.modules:
            self.modules[name] = ModuleFacts()
        return self.modules[name]

    def merge(self, other: TypeFacts) -> None:
        for name, module in other.modules.items():
            target = self.module(name)
            target.globals.update(module.globals)
            for func_name, func in module.functions.items():
                if func_name not in target.functions:
                    target.functions[func_name] = func
                else:
                    merged = target.functions[func_name]
                    merged.params.update(func.params)
                    merged.locals.update(func.locals)
                    if func.returns is not None:
                        merged.returns = func.returns

    def hints_for_globals(
        self, module_name: str, policy: TypeHintPolicy
    ) -> dict[str, str]:
        module = self.modules.get(module_name)
        if module is None:
            return {}
        return _filter_hints(module.globals, policy)

    def hints_for_function(
        self, module_name: str, func_name: str, policy: TypeHintPolicy
    ) -> dict[str, str]:
        module = self.modules.get(module_name)
        if module is None:
            return {}
        func = module.functions.get(func_name)
        if func is None:
            return {}
        hints: dict[str, str] = {}
        hints.update(_filter_hints(func.params, policy))
        hints.update(_filter_hints(func.locals, policy))
        return hints

    def to_dict(self) -> dict[str, Any]:
        def fact_dict(fact: Fact) -> dict[str, str]:
            return {"type": fact.type, "trust": fact.trust}

        modules: dict[str, Any] = {}
        for name, module in self.modules.items():
            functions: dict[str, Any] = {}
            for func_name, func in module.functions.items():
                functions[func_name] = {
                    "params": {k: fact_dict(v) for k, v in func.params.items()},
                    "locals": {k: fact_dict(v) for k, v in func.locals.items()},
                    "returns": fact_dict(func.returns) if func.returns else None,
                }
            modules[name] = {
                "globals": {k: fact_dict(v) for k, v in module.globals.items()},
                "functions": functions,
            }
        return {
            "schema_version": self.schema_version,
            "created_at": self.created_at,
            "tool": self.tool,
            "strict": self.strict,
            "modules": modules,
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> TypeFacts:
        facts = cls(
            schema_version=int(data.get("schema_version", 1)),
            created_at=str(data.get("created_at", "")) or cls().created_at,
            tool=str(data.get("tool", "molt-check")),
            strict=bool(data.get("strict", False)),
        )
        modules = data.get("modules", {})
        if isinstance(modules, dict):
            for name, module in modules.items():
                mod_facts = facts.module(str(name))
                globals_data = module.get("globals", {})
                if isinstance(globals_data, dict):
                    for key, val in globals_data.items():
                        fact = _fact_from_dict(val)
                        if fact:
                            mod_facts.globals[str(key)] = fact
                functions_data = module.get("functions", {})
                if isinstance(functions_data, dict):
                    for func_name, func_data in functions_data.items():
                        func_facts = FunctionFacts()
                        params = func_data.get("params", {})
                        if isinstance(params, dict):
                            for key, val in params.items():
                                fact = _fact_from_dict(val)
                                if fact:
                                    func_facts.params[str(key)] = fact
                        locals_data = func_data.get("locals", {})
                        if isinstance(locals_data, dict):
                            for key, val in locals_data.items():
                                fact = _fact_from_dict(val)
                                if fact:
                                    func_facts.locals[str(key)] = fact
                        returns_data = func_data.get("returns")
                        if isinstance(returns_data, dict):
                            func_facts.returns = _fact_from_dict(returns_data)
                        mod_facts.functions[str(func_name)] = func_facts
        return facts


def load_type_facts(path: Path) -> TypeFacts:
    data = json.loads(path.read_text())
    return TypeFacts.from_dict(data)


def write_type_facts(path: Path, facts: TypeFacts) -> None:
    path.write_text(json.dumps(facts.to_dict(), indent=2, sort_keys=True) + "\n")


def collect_type_facts_from_paths(
    paths: Iterable[Path], trust: TrustLevel, infer: bool = False
) -> TypeFacts:
    facts = TypeFacts(strict=(trust == "trusted"))
    for path in paths:
        module_name = path.stem
        source = path.read_text()
        module = facts.module(module_name)
        _collect_module_facts(module, source, trust, infer=infer)
    return facts


def normalize_type_hint(value: str | None) -> str | None:
    if value is None:
        return None
    text = value.strip()
    if not text:
        return None
    text = text.replace("typing.", "").replace("builtins.", "")
    if text.startswith("Literal[") and text.endswith("]"):
        inner = text[len("Literal[") : -1].strip()
        if inner.startswith(("'", '"')) and inner.endswith(("'", '"')):
            return "str"
        if inner in {"True", "False"}:
            return "bool"
        if inner.lstrip("-").isdigit():
            return "int"
        return "Any"
    if "|" in text or "Union[" in text or "Optional[" in text:
        return "Any"
    if "[" in text and text.endswith("]"):
        base, inner = text.split("[", 1)
        base = base.strip().lower()
        inner = inner[:-1].strip()
        inner = inner.replace("typing.", "").replace("builtins.", "")
        inner_lower = inner.lower()
        if base in {"list", "tuple", "set", "frozenset"}:
            inner_mapping = {
                "int": "int",
                "float": "float",
                "str": "str",
                "bytes": "bytes",
                "bytearray": "bytearray",
                "bool": "bool",
            }
            if inner_lower in inner_mapping:
                return f"{base}[{inner_mapping[inner_lower]}]"
            if base == "tuple" and "," in inner:
                parts = [part.strip() for part in inner.split(",") if part.strip()]
                if len(parts) == 2 and parts[1] in {"...", "Ellipsis"}:
                    elem = parts[0].lower()
                    if elem in inner_mapping:
                        return f"{base}[{inner_mapping[elem]}]"
            if inner_lower in {"any", "object"}:
                return base
            return None
        if base == "dict":
            if "," in inner:
                parts = [part.strip() for part in inner.split(",") if part.strip()]
                if len(parts) == 2:
                    key_lower = parts[0].lower()
                    val_lower = parts[1].lower()
                    if key_lower == "str":
                        value_mapping = {
                            "int": "int",
                            "float": "float",
                            "str": "str",
                            "bytes": "bytes",
                            "bytearray": "bytearray",
                            "bool": "bool",
                        }
                        val = value_mapping.get(val_lower)
                        if val:
                            return f"dict[str,{val}]"
            if inner_lower in {"any", "object"}:
                return "dict"
            return None
        text = base
    base = text.strip()
    base_lower = base.lower()
    mapping = {
        "int": "int",
        "float": "float",
        "str": "str",
        "bytes": "bytes",
        "bytearray": "bytearray",
        "bool": "bool",
        "list": "list",
        "tuple": "tuple",
        "dict": "dict",
        "set": "set",
        "frozenset": "frozenset",
        "range": "range",
        "slice": "slice",
        "memoryview": "memoryview",
        "none": "None",
        "nonetype": "None",
        "any": "Any",
        "object": "Any",
    }
    if base_lower == "buffer2d" and base == base_lower:
        return "buffer2d"
    return mapping.get(base_lower, base)


def _infer_expr_type(node: ast.expr) -> str | None:
    if isinstance(node, ast.Constant):
        value = node.value
        if isinstance(value, bool):
            return "bool"
        if isinstance(value, int):
            return "int"
        if isinstance(value, float):
            return "float"
        if isinstance(value, str):
            return "str"
        if isinstance(value, bytes):
            return "bytes"
        if value is None:
            return "None"
        return None
    if isinstance(node, ast.List):
        if not node.elts:
            return "list"
        elem_types = {_infer_expr_type(elt) for elt in node.elts}
        if None in elem_types or len(elem_types) != 1:
            return "list"
        elem = elem_types.pop()
        if elem in {"int", "float", "str", "bytes", "bytearray", "bool"}:
            return f"list[{elem}]"
        return "list"
    if isinstance(node, ast.Tuple):
        if not node.elts:
            return "tuple"
        elem_types = {_infer_expr_type(elt) for elt in node.elts}
        if None in elem_types or len(elem_types) != 1:
            return "tuple"
        elem = elem_types.pop()
        if elem in {"int", "float", "str", "bytes", "bytearray", "bool"}:
            return f"tuple[{elem}]"
        return "tuple"
    if isinstance(node, ast.Dict):
        if not node.keys:
            return "dict"
        key_types = {_infer_expr_type(key) for key in node.keys if key is not None}
        val_types = {_infer_expr_type(val) for val in node.values}
        if None in key_types or None in val_types:
            return "dict"
        if key_types == {"str"} and len(val_types) == 1:
            val = next(iter(val_types))
            if val in {"int", "float", "str", "bytes", "bytearray", "bool"}:
                return f"dict[str,{val}]"
        return "dict"
    return None


def _fact_from_dict(data: Any) -> Fact | None:
    if not isinstance(data, dict):
        return None
    type_text = data.get("type")
    trust = data.get("trust", "guarded")
    if not isinstance(type_text, str) or not isinstance(trust, str):
        return None
    if normalize_type_hint(type_text) is None:
        return None
    if trust not in {"advisory", "guarded", "trusted"}:
        trust = "guarded"
    return Fact(type_text, trust)  # type: ignore[arg-type]


def _filter_hints(facts: dict[str, Fact], policy: TypeHintPolicy) -> dict[str, str]:
    if policy == "ignore":
        return {}
    allowed = {"guarded", "trusted"} if policy == "check" else {"trusted"}
    hints: dict[str, str] = {}
    for name, fact in facts.items():
        if fact.trust in allowed:
            normalized = normalize_type_hint(fact.type)
            if normalized is not None:
                hints[name] = normalized
    return hints


def _annotation_to_string(node: ast.expr) -> str | None:
    try:
        return ast.unparse(node)
    except Exception:
        return None


def _collect_module_facts(
    module: ModuleFacts, source: str, trust: TrustLevel, infer: bool
) -> None:
    tree = ast.parse(source)
    for stmt in tree.body:
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            module.functions[stmt.name] = _collect_function_facts(
                stmt, trust, infer=infer
            )
        elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name):
            hint = _annotation_to_string(stmt.annotation)
            normalized = normalize_type_hint(hint)
            if normalized and hint is not None:
                module.globals[stmt.target.id] = Fact(hint, trust)
        elif infer and isinstance(stmt, ast.Assign):
            if len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name):
                name = stmt.targets[0].id
                if name not in module.globals:
                    inferred = _infer_expr_type(stmt.value)
                    if inferred:
                        module.globals[name] = Fact(inferred, trust)


def _collect_function_facts(
    node: ast.FunctionDef | ast.AsyncFunctionDef, trust: TrustLevel, infer: bool
) -> FunctionFacts:
    facts = FunctionFacts()
    for arg in node.args.args:
        if arg.annotation is None:
            continue
        hint = _annotation_to_string(arg.annotation)
        normalized = normalize_type_hint(hint)
        if normalized and hint is not None:
            facts.params[arg.arg] = Fact(hint, trust)
    if node.returns is not None:
        hint = _annotation_to_string(node.returns)
        normalized = normalize_type_hint(hint)
        if normalized and hint is not None:
            facts.returns = Fact(hint, trust)

    stack: list[ast.AST] = list(node.body)
    while stack:
        inner = stack.pop()
        if isinstance(inner, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue
        if isinstance(inner, ast.AnnAssign) and isinstance(inner.target, ast.Name):
            hint = _annotation_to_string(inner.annotation)
            normalized = normalize_type_hint(hint)
            if normalized and hint is not None:
                facts.locals[inner.target.id] = Fact(hint, trust)
        elif infer and isinstance(inner, ast.Assign):
            if (
                len(inner.targets) == 1
                and isinstance(inner.targets[0], ast.Name)
                and inner.targets[0].id not in facts.locals
            ):
                inferred = _infer_expr_type(inner.value)
                if inferred:
                    facts.locals[inner.targets[0].id] = Fact(inferred, trust)
        stack.extend(ast.iter_child_nodes(inner))
    return facts
