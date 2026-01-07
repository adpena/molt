from __future__ import annotations

import ast
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Callable, Literal, TypedDict, cast

from molt.compat import CompatibilityError, CompatibilityReporter, FallbackPolicy
from molt.type_facts import normalize_type_hint

if TYPE_CHECKING:
    from molt.type_facts import TypeFacts


@dataclass
class MoltValue:
    name: str
    type_hint: str = "Unknown"


@dataclass
class MoltOp:
    kind: str
    args: list[Any]
    result: MoltValue
    metadata: dict[str, Any] | None = None


GEN_SEND_OFFSET = 0
GEN_THROW_OFFSET = 8
GEN_CLOSED_OFFSET = 16
GEN_CONTROL_SIZE = 32

BUILTIN_TYPE_TAGS = {
    "int": 1,
    "float": 2,
    "bool": 3,
    "str": 5,
    "bytes": 6,
    "bytearray": 7,
    "list": 8,
    "tuple": 9,
    "dict": 10,
    "range": 11,
    "slice": 12,
    "memoryview": 15,
    "object": 100,
    "type": 101,
}


@dataclass
class TryScope:
    ctx_mark: MoltValue
    finalbody: list[ast.stmt] | None
    ctx_mark_offset: int | None = None


class MethodInfo(TypedDict):
    func: MoltValue
    attr: MoltValue
    descriptor: Literal["function", "classmethod", "staticmethod", "property"]
    return_hint: str | None


class ClassInfo(TypedDict, total=False):
    fields: dict[str, int]
    size: int
    field_order: list[str]
    defaults: dict[str, ast.expr]
    class_attrs: dict[str, ast.expr]
    base: str | None
    bases: list[str]
    mro: list[str]
    dynamic: bool
    dataclass: bool
    frozen: bool
    eq: bool
    repr: bool
    slots: bool
    methods: dict[str, MethodInfo]


class FuncInfo(TypedDict):
    params: list[str]
    ops: list[MoltOp]


class SimpleTIRGenerator(ast.NodeVisitor):
    def __init__(
        self,
        parse_codec: Literal["msgpack", "cbor", "json"] = "msgpack",
        type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
        fallback_policy: FallbackPolicy = "error",
        source_path: str | None = None,
        type_facts: "TypeFacts | None" = None,
        module_name: str | None = None,
        entry_module: str | None = None,
        enable_phi: bool = True,
        known_modules: set[str] | None = None,
        stdlib_allowlist: set[str] | None = None,
    ) -> None:
        self.funcs_map: dict[str, FuncInfo] = {"molt_main": {"params": [], "ops": []}}
        self.current_func_name: str = "molt_main"
        self.current_ops: list[MoltOp] = self.funcs_map["molt_main"]["ops"]
        self.var_count: int = 0
        self.state_count: int = 0
        self.classes: dict[str, ClassInfo] = {}
        self.locals: dict[str, MoltValue] = {}
        self.boxed_locals: dict[str, MoltValue] = {}
        self.boxed_local_hints: dict[str, str] = {}
        self.global_decls: set[str] = set()
        self.exact_locals: dict[str, str] = {}
        self.globals: dict[str, MoltValue] = {}
        self.func_symbol_names: dict[str, str] = {}
        self.async_locals: dict[str, int] = {}
        self.async_locals_base: int = 0
        self.async_local_hints: dict[str, str] = {}
        self.parse_codec = parse_codec
        self.type_hint_policy = type_hint_policy
        self.explicit_type_hints: dict[str, str] = {}
        self.container_elem_hints: dict[str, str] = {}
        self.global_elem_hints: dict[str, str] = {}
        self.dict_key_hints: dict[str, str] = {}
        self.dict_value_hints: dict[str, str] = {}
        self.global_dict_key_hints: dict[str, str] = {}
        self.global_dict_value_hints: dict[str, str] = {}
        self.type_facts = type_facts
        self.module_name = module_name or "__main__"
        self.entry_module = entry_module
        self.enable_phi = enable_phi
        self.module_prefix = f"{self._sanitize_module_name(self.module_name)}__"
        self.known_modules = set(known_modules or [])
        self.stdlib_allowlist = set(stdlib_allowlist or [])
        self.module_obj: MoltValue | None = None
        self.defer_module_attrs = False
        self.deferred_module_attrs: set[str] = set()
        self.fallback_policy = fallback_policy
        self.compat = CompatibilityReporter(fallback_policy, source_path)
        self.context_depth = 0
        self.try_end_labels: list[int] = []
        self.try_scopes: list[TryScope] = []
        self.try_suppress_depth: int | None = None
        self.return_unwind_depth = 0
        self.return_label: int | None = None
        self.return_slot: MoltValue | None = None
        self.return_slot_index: MoltValue | None = None
        self.return_slot_offset: int | None = None
        self.active_exceptions: list[MoltValue] = []
        self.func_aliases: dict[str, str] = {}
        self.const_ints: dict[str, int] = {}
        self.in_generator = False
        self.current_class: str | None = None
        self.current_method_first_param: str | None = None
        if self.module_name:
            name_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=[self.module_name], result=name_val)
            )
            module_val = MoltValue(self.next_var(), type_hint="module")
            self.emit(MoltOp(kind="MODULE_NEW", args=[name_val], result=module_val))
            self.emit(
                MoltOp(
                    kind="MODULE_CACHE_SET",
                    args=[name_val, module_val],
                    result=MoltValue("none"),
                )
            )
            self.module_obj = module_val
        self._apply_type_facts("molt_main")

    def _c3_merge(self, seqs: list[list[str]]) -> list[str] | None:
        merged: list[str] = []
        working = [list(seq) for seq in seqs]
        while True:
            working = [seq for seq in working if seq]
            if not working:
                return merged
            candidate = None
            for seq in working:
                head = seq[0]
                if any(head in tail[1:] for tail in working):
                    continue
                candidate = head
                break
            if candidate is None:
                return None
            merged.append(candidate)
            for seq in working:
                if seq and seq[0] == candidate:
                    seq.pop(0)

    def _class_mro_names(self, name: str) -> list[str]:
        if name == "object":
            return ["object"]
        info = self.classes.get(name)
        if info is None:
            return [name]
        cached = info.get("mro")
        if cached:
            return cached
        bases = info.get("bases", [])
        seqs = [self._class_mro_names(base) for base in bases]
        seqs.append(list(bases))
        merged = self._c3_merge(seqs)
        if merged is None:
            raise NotImplementedError(
                "Cannot create a consistent method resolution order (MRO) for bases"
            )
        mro = [name] + merged
        info["mro"] = mro
        return mro

    def _resolve_method_info(
        self, class_name: str, method: str
    ) -> tuple[MethodInfo | None, str | None]:
        for name in self._class_mro_names(class_name):
            info = self.classes.get(name)
            if info and "methods" in info and method in info["methods"]:
                return info["methods"][method], name
        return None, None

    def _resolve_super_method_info(
        self, class_name: str, method: str
    ) -> tuple[MethodInfo | None, str | None]:
        mro = self._class_mro_names(class_name)
        found_start = False
        for name in mro:
            if not found_start:
                if name == class_name:
                    found_start = True
                continue
            info = self.classes.get(name)
            if info and "methods" in info and method in info["methods"]:
                return info["methods"][method], name
        return None, None

    def visit(self, node: ast.AST) -> Any:
        try:
            return super().visit(node)
        except CompatibilityError:
            raise
        except NotImplementedError as exc:
            raise self.compat.unsupported(
                node,
                feature=str(exc),
                tier="bridge",
                impact="high",
            ) from exc

    def next_var(self) -> str:
        name = f"v{self.var_count}"
        self.var_count += 1
        return name

    def next_label(self) -> int:
        self.state_count += 1
        return self.state_count

    def emit(self, op: MoltOp) -> None:
        if (
            op.kind == "CONST"
            and op.result
            and isinstance(op.args[0], int)
            and not isinstance(op.args[0], bool)
        ):
            self.const_ints[op.result.name] = op.args[0]
        self.current_ops.append(op)
        if not self.try_end_labels:
            return
        if (
            self.try_suppress_depth is not None
            and len(self.try_end_labels) <= self.try_suppress_depth
        ):
            return
        if op.kind in {
            "CHECK_EXCEPTION",
            "TRY_START",
            "TRY_END",
            "LABEL",
            "STATE_LABEL",
            "JUMP",
            "BR_IF",
            "IF",
            "ELSE",
            "END_IF",
            "LOOP_START",
            "LOOP_END",
            "LOOP_CONTINUE",
            "LOOP_BREAK_IF_TRUE",
            "LOOP_BREAK_IF_FALSE",
            "LOOP_INDEX_START",
            "LOOP_INDEX_NEXT",
            "STATE_TRANSITION",
            "STATE_YIELD",
            "PHI",
            "EXCEPTION_PUSH",
            "EXCEPTION_POP",
            "EXCEPTION_CLEAR",
            "EXCEPTION_LAST",
            "EXCEPTION_SET_CAUSE",
            "EXCEPTION_CONTEXT_SET",
            "CONTEXT_UNWIND_TO",
            "ret",
        }:
            return
        handler_label = self.try_end_labels[-1]
        self.current_ops.append(
            MoltOp(
                kind="CHECK_EXCEPTION",
                args=[handler_label],
                result=MoltValue("none"),
            )
        )

    def _fast_int_enabled(self) -> bool:
        return self.type_hint_policy in {"trust", "check"}

    def _should_fast_int(self, op: MoltOp) -> bool:
        if not self._fast_int_enabled():
            return False
        if op.kind not in {"ADD", "SUB", "MUL", "LT", "EQ"}:
            return False
        return all(
            isinstance(arg, MoltValue) and arg.type_hint == "int" for arg in op.args
        )

    def _emit_bridge_unavailable(self, message: str) -> MoltValue:
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="BRIDGE_UNAVAILABLE", args=[msg_val], result=res))
        return res

    def _bridge_fallback(
        self,
        node: ast.AST,
        feature: str,
        *,
        impact: Literal["low", "medium", "high"] = "high",
        alternative: str | None = None,
        detail: str | None = None,
    ) -> MoltValue:
        issue = self.compat.bridge_unavailable(
            node, feature, impact=impact, alternative=alternative, detail=detail
        )
        if self.fallback_policy != "bridge":
            raise self.compat.error(issue)
        return self._emit_bridge_unavailable(issue.runtime_message())

    def _emit_nullcontext(self, payload: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="context_manager")
        self.emit(MoltOp(kind="CONTEXT_NULL", args=[payload], result=res))
        return res

    def _emit_closing(self, payload: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="context_manager")
        self.emit(MoltOp(kind="CONTEXT_CLOSING", args=[payload], result=res))
        return res

    def _is_contextmanager_decorator(self, deco: ast.expr) -> bool:
        if isinstance(deco, ast.Name) and deco.id == "contextmanager":
            return True
        if (
            isinstance(deco, ast.Attribute)
            and isinstance(deco.value, ast.Name)
            and deco.value.id == "contextlib"
            and deco.attr == "contextmanager"
        ):
            return True
        return False

    @staticmethod
    def _sanitize_module_name(name: str) -> str:
        out: list[str] = []
        for ch in name:
            if ch.isalnum() or ch == "_":
                out.append(ch)
            else:
                out.append("_")
        if not out:
            return "module"
        return "".join(out)

    @classmethod
    def module_init_symbol(cls, name: str) -> str:
        return f"molt_init_{cls._sanitize_module_name(name)}"

    @staticmethod
    def _function_contains_yield(node: ast.FunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, (ast.Yield, ast.YieldFrom)):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _function_contains_return(node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, ast.Return):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    def _function_symbol(self, name: str) -> str:
        if name in self.func_aliases:
            return self.func_aliases[name]
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while symbol in self.funcs_map:
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.func_aliases[name] = symbol
        self.func_symbol_names[symbol] = name
        return symbol

    def start_function(
        self,
        name: str,
        params: list[str] | None = None,
        type_facts_name: str | None = None,
        needs_return_slot: bool = False,
    ) -> None:
        if name not in self.funcs_map:
            self.funcs_map[name] = FuncInfo(params=params or [], ops=[])
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]
        self.locals = {}
        self.boxed_locals = {}
        self.boxed_local_hints = {}
        self.global_decls = set()
        self.exact_locals = {}
        self.async_locals = {}
        self.async_locals_base = 0
        self.async_local_hints = {}
        self.explicit_type_hints = {}
        self.container_elem_hints = {}
        self.dict_key_hints = {}
        self.dict_value_hints = {}
        self.context_depth = 0
        self.const_ints = {}
        self.in_generator = False
        self.try_end_labels = []
        self.try_scopes = []
        self.try_suppress_depth = None
        self.return_unwind_depth = 0
        self.active_exceptions = []
        self.return_label = None
        self.return_slot = None
        self.return_slot_index = None
        self.return_slot_offset = None
        if needs_return_slot:
            self._init_return_slot()
        self._apply_type_facts(type_facts_name or name)

    def _module_can_defer_attrs(self, node: ast.Module) -> bool:
        for current in ast.walk(node):
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                return False
            if isinstance(current, ast.Call) and isinstance(current.func, ast.Name):
                if current.func.id in {"globals", "locals", "vars"}:
                    return False
        return True

    def _flush_deferred_module_attrs(self) -> None:
        if not self.deferred_module_attrs or self.module_obj is None:
            return
        for name in sorted(self.deferred_module_attrs):
            val = self._load_local_value(name)
            if val is None:
                val = self.globals.get(name)
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self._emit_module_attr_set_on(self.module_obj, name, val)

    def _capture_function_state(self) -> dict[str, Any]:
        return {
            "locals": self.locals,
            "boxed_locals": self.boxed_locals,
            "boxed_local_hints": self.boxed_local_hints,
            "global_decls": self.global_decls,
            "exact_locals": self.exact_locals,
            "async_locals": self.async_locals,
            "async_locals_base": self.async_locals_base,
            "async_local_hints": self.async_local_hints,
            "explicit_type_hints": self.explicit_type_hints,
            "container_elem_hints": self.container_elem_hints,
            "dict_key_hints": self.dict_key_hints,
            "dict_value_hints": self.dict_value_hints,
            "context_depth": self.context_depth,
            "const_ints": self.const_ints,
            "in_generator": self.in_generator,
            "try_end_labels": self.try_end_labels,
            "try_scopes": self.try_scopes,
            "try_suppress_depth": self.try_suppress_depth,
            "return_unwind_depth": self.return_unwind_depth,
            "active_exceptions": self.active_exceptions,
            "return_label": self.return_label,
            "return_slot": self.return_slot,
            "return_slot_index": self.return_slot_index,
            "return_slot_offset": self.return_slot_offset,
            "defer_module_attrs": self.defer_module_attrs,
            "deferred_module_attrs": self.deferred_module_attrs,
        }

    def _restore_function_state(self, state: dict[str, Any]) -> None:
        self.locals = state["locals"]
        self.boxed_locals = state["boxed_locals"]
        self.boxed_local_hints = state["boxed_local_hints"]
        self.global_decls = state["global_decls"]
        self.exact_locals = state["exact_locals"]
        self.async_locals = state["async_locals"]
        self.async_locals_base = state["async_locals_base"]
        self.async_local_hints = state["async_local_hints"]
        self.explicit_type_hints = state["explicit_type_hints"]
        self.container_elem_hints = state["container_elem_hints"]
        self.dict_key_hints = state["dict_key_hints"]
        self.dict_value_hints = state["dict_value_hints"]
        self.context_depth = state["context_depth"]
        self.const_ints = state["const_ints"]
        self.in_generator = state["in_generator"]
        self.try_end_labels = state["try_end_labels"]
        self.try_scopes = state["try_scopes"]
        self.try_suppress_depth = state["try_suppress_depth"]
        self.return_unwind_depth = state["return_unwind_depth"]
        self.active_exceptions = state["active_exceptions"]
        self.return_label = state["return_label"]
        self.return_slot = state["return_slot"]
        self.return_slot_index = state["return_slot_index"]
        self.return_slot_offset = state["return_slot_offset"]
        self.defer_module_attrs = state["defer_module_attrs"]
        self.deferred_module_attrs = state["deferred_module_attrs"]

    def visit_Module(self, node: ast.Module) -> None:
        defer = self._module_can_defer_attrs(node)
        prev_defer = self.defer_module_attrs
        prev_dirty = self.deferred_module_attrs
        if defer:
            self.defer_module_attrs = True
            self.deferred_module_attrs = set()
        for stmt in node.body:
            self.visit(stmt)
        if defer:
            self._flush_deferred_module_attrs()
        self.defer_module_attrs = prev_defer
        self.deferred_module_attrs = prev_dirty
        return None

    def _init_return_slot(self) -> None:
        if self.return_label is not None:
            return
        self.return_label = self.next_label()
        self.return_slot_index = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=self.return_slot_index))
        init = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
        self.return_slot = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[init], result=self.return_slot))

    def _store_return_slot_for_stateful(self) -> None:
        if not self.is_async() or self.return_slot is None:
            return
        if self.return_slot_offset is None:
            self.return_slot_offset = self._async_local_offset("__molt_return_slot")
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", self.return_slot_offset, self.return_slot],
                result=MoltValue("none"),
            )
        )

    def _load_return_slot(self) -> MoltValue | None:
        if self.return_slot is None:
            return None
        if self.is_async() and self.return_slot_offset is not None:
            slot_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.return_slot_offset],
                    result=slot_val,
                )
            )
            return slot_val
        return self.return_slot

    def _load_return_slot_index(self) -> MoltValue:
        if self.is_async():
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            return idx
        idx = self.return_slot_index
        if idx is None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.return_slot_index = idx
        return idx

    def _emit_return_value(self, value: MoltValue) -> None:
        if self.return_slot is None or self.return_label is None:
            self.emit(MoltOp(kind="ret", args=[value], result=MoltValue("none")))
            return
        slot = self._load_return_slot()
        if slot is None:
            self.emit(MoltOp(kind="ret", args=[value], result=MoltValue("none")))
            return
        idx = self._load_return_slot_index()
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[slot, idx, value],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(kind="JUMP", args=[self.return_label], result=MoltValue("none"))
        )

    def _emit_return_label(self) -> None:
        if self.return_label is None or self.return_slot is None:
            return
        self.emit(
            MoltOp(kind="LABEL", args=[self.return_label], result=MoltValue("none"))
        )
        slot = self._load_return_slot()
        if slot is None:
            return
        idx = self._load_return_slot_index()
        res = MoltValue(self.next_var())
        self.emit(MoltOp(kind="INDEX", args=[slot, idx], result=res))
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))

    def _ends_with_return_jump(self) -> bool:
        if not self.current_ops:
            return False
        last = self.current_ops[-1]
        if last.kind == "ret":
            return True
        if (
            last.kind == "JUMP"
            and self.return_label is not None
            and last.args
            and last.args[0] == self.return_label
        ):
            return True
        return False

    def resume_function(self, name: str) -> None:
        self.current_func_name = name
        self.current_ops = self.funcs_map[name]["ops"]

    def is_async(self) -> bool:
        return self.current_func_name.endswith("_poll")

    def _parse_container_hint(self, hint: str) -> tuple[str, str | None]:
        if hint.endswith("]") and "[" in hint:
            base, inner = hint.split("[", 1)
            base = base.strip()
            inner = inner[:-1].strip()
            if base in {"list", "tuple"} and inner:
                if "," in inner:
                    parts = [part.strip() for part in inner.split(",") if part.strip()]
                    if parts:
                        inner = parts[0]
                return base, inner
            if base == "dict":
                return base, None
        return hint, None

    def _parse_dict_hint(self, hint: str) -> tuple[str | None, str | None]:
        if not hint.startswith("dict[") or not hint.endswith("]"):
            return None, None
        inner = hint[len("dict[") : -1]
        parts = [part.strip() for part in inner.split(",") if part.strip()]
        if len(parts) != 2:
            return None, None
        return parts[0], parts[1]

    def _expr_is_data_descriptor(self, expr: ast.expr) -> bool:
        if isinstance(expr, ast.Call) and isinstance(expr.func, ast.Name):
            if expr.func.id == "property":
                return True
            class_info = self.classes.get(expr.func.id)
            if class_info:
                methods = class_info.get("methods", {})
                return "__set__" in methods or "__delete__" in methods
        return False

    def _class_attr_is_data_descriptor(self, class_name: str, attr: str) -> bool:
        class_info = self.classes.get(class_name)
        if not class_info:
            return False
        for mro_name in class_info.get("mro", [class_name]):
            mro_info = self.classes.get(mro_name)
            if not mro_info:
                continue
            class_attrs = mro_info.get("class_attrs", {})
            expr = class_attrs.get(attr)
            if expr is not None and self._expr_is_data_descriptor(expr):
                return True
            method_info = mro_info.get("methods", {}).get(attr)
            if method_info and method_info["descriptor"] == "property":
                return True
        return False

    def _async_local_offset(self, name: str) -> int:
        if name not in self.async_locals:
            self.async_locals[name] = (
                self.async_locals_base + len(self.async_locals) * 8
            )
        return self.async_locals[name]

    def _apply_hint_to_value(
        self, _name: str | None, value: MoltValue, hint: str
    ) -> None:
        base, elem = self._parse_container_hint(hint)
        value.type_hint = base
        if self.current_func_name == "molt_main":
            elem_target = self.global_elem_hints
            key_target = self.global_dict_key_hints
            val_target = self.global_dict_value_hints
        else:
            elem_target = self.container_elem_hints
            key_target = self.dict_key_hints
            val_target = self.dict_value_hints
        key = value.name
        if base == "dict":
            dict_key, dict_val = self._parse_dict_hint(hint)
            if dict_key and dict_val:
                key_target[key] = dict_key
                val_target[key] = dict_val
            else:
                key_target.pop(key, None)
                val_target.pop(key, None)
            elem_target.pop(key, None)
        else:
            if elem:
                elem_target[key] = elem
            else:
                elem_target.pop(key, None)
            key_target.pop(key, None)
            val_target.pop(key, None)

    def _propagate_container_hints(self, dest: str, src: MoltValue) -> None:
        if self.current_func_name == "molt_main":
            elem_map = self.global_elem_hints
            key_map = self.global_dict_key_hints
            val_map = self.global_dict_value_hints
        else:
            elem_map = self.container_elem_hints
            key_map = self.dict_key_hints
            val_map = self.dict_value_hints
        if src.name in elem_map:
            elem_map[dest] = elem_map[src.name]
        else:
            elem_map.pop(dest, None)
        if src.name in key_map and src.name in val_map:
            key_map[dest] = key_map[src.name]
            val_map[dest] = val_map[src.name]
        else:
            key_map.pop(dest, None)
            val_map.pop(dest, None)

    def _container_elem_hint(self, value: MoltValue) -> str | None:
        if value.name in self.container_elem_hints:
            return self.container_elem_hints[value.name]
        return self.global_elem_hints.get(value.name)

    def _dict_value_hint(self, value: MoltValue) -> str | None:
        if value.name in self.dict_value_hints:
            return self.dict_value_hints[value.name]
        return self.global_dict_value_hints.get(value.name)

    def _apply_type_facts(self, func_name: str) -> None:
        if self.type_facts is None:
            return
        if func_name == "molt_main":
            hints = self.type_facts.hints_for_globals(
                self.module_name, self.type_hint_policy
            )
        else:
            hints = self.type_facts.hints_for_function(
                self.module_name, func_name, self.type_hint_policy
            )
        self.explicit_type_hints.update(hints)

    def _annotation_to_hint(self, node: ast.expr | None) -> str | None:
        if node is None:
            return None
        try:
            text = ast.unparse(node)
        except Exception:
            return None
        return normalize_type_hint(text)

    def _guard_tag_for_hint(self, hint: str) -> int | None:
        mapping = {
            "Any": 0,
            "Unknown": 0,
            "int": 1,
            "float": 2,
            "bool": 3,
            "None": 4,
            "str": 5,
            "bytes": 6,
            "bytearray": 7,
            "list": 8,
            "tuple": 9,
            "dict": 10,
            "range": 11,
            "slice": 12,
            "dataclass": 13,
            "buffer2d": 14,
            "memoryview": 15,
        }
        return mapping.get(hint)

    def _emit_guard_type(self, value: MoltValue, hint: str) -> None:
        base = hint.split("[", 1)[0] if "[" in hint else hint
        tag = self._guard_tag_for_hint(base)
        if tag is None or tag == 0:
            return
        tag_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[tag], result=tag_val))
        self.emit(
            MoltOp(kind="GUARD_TYPE", args=[value, tag_val], result=MoltValue("none"))
        )

    def _emit_module_attr_set(self, name: str, value: MoltValue) -> None:
        if self.current_func_name != "molt_main" or self.module_obj is None:
            return
        if self.defer_module_attrs:
            self.deferred_module_attrs.add(name)
            return
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[self.module_obj, name_val, value],
                result=MoltValue("none"),
            )
        )

    def _emit_module_attr_set_on(
        self, module_val: MoltValue, name: str, value: MoltValue
    ) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )

    def _emit_function_metadata(
        self,
        func_val: MoltValue,
        *,
        name: str,
        qualname: str,
        params: list[str],
        default_exprs: list[ast.expr],
        docstring: str | None,
        is_coroutine: bool = False,
        is_generator: bool = False,
    ) -> None:
        def set_attr(attr: str, value: MoltValue) -> None:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, attr, value],
                    result=MoltValue("none"),
                )
            )

        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        set_attr("__name__", name_val)

        qual_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[qualname], result=qual_val))
        set_attr("__qualname__", qual_val)

        module_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[self.module_name], result=module_val))
        set_attr("__module__", module_val)

        arg_name_vals: list[MoltValue] = []
        for param in params:
            param_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[param], result=param_val))
            arg_name_vals.append(param_val)
        arg_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=arg_name_vals, result=arg_names_tuple))
        set_attr("__molt_arg_names__", arg_names_tuple)

        if default_exprs:
            default_vals: list[MoltValue] = []
            for expr in default_exprs:
                val = self.visit(expr)
                if val is None:
                    val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                default_vals.append(val)
            defaults_tuple = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(kind="TUPLE_NEW", args=default_vals, result=defaults_tuple)
            )
            set_attr("__defaults__", defaults_tuple)
        else:
            defaults_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=defaults_none))
            set_attr("__defaults__", defaults_none)

        if docstring is None:
            doc_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=doc_val))
        else:
            doc_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[docstring], result=doc_val))
        set_attr("__doc__", doc_val)

        if is_coroutine:
            coro_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=coro_val))
            set_attr("__molt_is_coroutine__", coro_val)
        if is_generator:
            gen_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gen_val))
            set_attr("__molt_is_generator__", gen_val)

    def _emit_module_attr_get(self, name: str) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_name = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[self.module_name], result=module_name))
        module_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(kind="MODULE_CACHE_GET", args=[module_name], result=module_val)
        )
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, name_val], result=res)
        )
        return res

    def _emit_module_attr_set_runtime(self, name: str, value: MoltValue) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_name = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[self.module_name], result=module_name))
        module_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(kind="MODULE_CACHE_GET", args=[module_name], result=module_val)
        )
        self.emit(
            MoltOp(
                kind="MODULE_SET_ATTR",
                args=[module_val, name_val, value],
                result=MoltValue("none"),
            )
        )

    def _emit_module_load(self, module_name: str) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=name_val))
        module_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(MoltOp(kind="MODULE_CACHE_GET", args=[name_val], result=module_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[module_val, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        if module_name in self.known_modules:
            init_symbol = self.module_init_symbol(module_name)
            init_res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="CALL", args=[init_symbol], result=init_res))
        elif module_name in self.stdlib_allowlist:
            stub_val = MoltValue(self.next_var(), type_hint="module")
            self.emit(MoltOp(kind="MODULE_NEW", args=[name_val], result=stub_val))
            self.emit(
                MoltOp(
                    kind="MODULE_CACHE_SET",
                    args=[name_val, stub_val],
                    result=MoltValue("none"),
                )
            )
        elif self.known_modules:
            exc_val = self._emit_exception_new(
                "ImportError", f"No module named '{module_name}'"
            )
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        loaded_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(MoltOp(kind="MODULE_CACHE_GET", args=[name_val], result=loaded_val))
        self._emit_import_guard(loaded_val, module_name)
        return loaded_val

    def _emit_module_load_with_parents(self, module_name: str) -> MoltValue:
        parts = module_name.split(".")
        parent_val: MoltValue | None = None
        current_val: MoltValue | None = None
        for idx, part in enumerate(parts):
            name = ".".join(parts[: idx + 1])
            current_val = self._emit_module_load(name)
            if parent_val is not None:
                self._emit_module_attr_set_on(parent_val, part, current_val)
            parent_val = current_val
        if current_val is None:
            raise NotImplementedError("Invalid module name")
        return current_val

    def _emit_import_guard(self, module_val: MoltValue, module_name: str) -> None:
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[module_val, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        exc_val = self._emit_exception_new(
            "ImportError", f"No module named '{module_name}'"
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_exception_new(self, kind: str, message: str) -> MoltValue:
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[kind], result=kind_val))
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, msg_val],
                result=exc_val,
            )
        )
        return exc_val

    def _emit_stop_iteration_from_value(self, value: MoltValue) -> None:
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        empty_msg = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=empty_msg))
        msg_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[empty_msg], result=msg_cell))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, none_val], result=is_none))
        not_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=not_none))
        self.emit(MoltOp(kind="IF", args=[not_none], result=MoltValue("none")))
        if value.type_hint == "str":
            msg_val = value
        else:
            msg_val = self._emit_str_from_obj(value)
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[msg_cell, zero, msg_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        final_msg = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="INDEX", args=[msg_cell, zero], result=final_msg))
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["StopIteration"], result=kind_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, final_msg],
                result=exc_val,
            )
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))

    def _emit_exception_match(
        self, handler: ast.ExceptHandler, exc_val: MoltValue
    ) -> MoltValue:
        if handler.type is None:
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[1], result=res))
            return res
        if isinstance(handler.type, ast.Name):
            name = handler.type.id
            if name in {"Exception", "BaseException"}:
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[1], result=res))
                return res
            kind_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="EXCEPTION_KIND", args=[exc_val], result=kind_val))
            expected = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=expected))
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="EQ", args=[kind_val, expected], result=res))
            return res
        self._bridge_fallback(
            handler,
            "except (non-name handler)",
            alternative="use bare except or a single name",
            detail="tuple and expression handlers are not supported yet",
        )
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[0], result=res))
        return res

    def _apply_explicit_hint(self, name: str, value: MoltValue) -> None:
        hint = self.explicit_type_hints.get(name)
        if hint is None:
            return
        if self.type_hint_policy == "check":
            self._emit_guard_type(value, hint)
            self._apply_hint_to_value(name, value, hint)
            return
        if self.type_hint_policy == "trust":
            self._apply_hint_to_value(name, value, hint)

    def visit_Name(self, node: ast.Name) -> Any:
        if isinstance(node.ctx, ast.Load):
            if node.id == "__name__":
                module_name = (
                    "__main__"
                    if self.entry_module and self.module_name == self.entry_module
                    else self.module_name
                )
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=res))
                return res
            local = self._load_local_value(node.id)
            if local is not None:
                return local
            global_val = self.globals.get(node.id)
            if global_val is None:
                if node.id == "TYPE_CHECKING":
                    res = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[0], result=res))
                    return res
                builtin_tag = BUILTIN_TYPE_TAGS.get(node.id)
                if builtin_tag is not None:
                    tag_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[builtin_tag], result=tag_val))
                    res = MoltValue(self.next_var(), type_hint="type")
                    self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=res))
                    return res
                return None
            if self.current_func_name == "molt_main":
                return global_val
            return self._emit_module_attr_get(node.id)
        return node.id

    def visit_Global(self, node: ast.Global) -> None:
        if self.current_func_name == "molt_main":
            return None
        self.global_decls.update(node.names)
        return None

    def _box_local(self, name: str) -> None:
        if name in self.global_decls:
            return
        if name in self.boxed_locals:
            return
        if name in self.locals:
            init = self.locals[name]
        else:
            init = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[init], result=cell))
        self.boxed_locals[name] = cell
        if init.type_hint:
            self.boxed_local_hints[name] = init.type_hint
        else:
            self.boxed_local_hints[name] = "Unknown"
        self.locals[name] = cell

    def _collect_assigned_names(self, nodes: list[ast.stmt]) -> set[str]:
        class AssignCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        self.names.add(target.id)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name):
                    self.names.add(node.target.id)
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                if isinstance(node.target, ast.Name):
                    self.names.add(node.target.id)
                self.generic_visit(node.value)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AssignCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_global_decls(self, nodes: list[ast.stmt]) -> set[str]:
        class GlobalCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Global(self, node: ast.Global) -> None:
                self.names.update(node.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = GlobalCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _class_id_from_call(self, node: ast.Call) -> str | None:
        if isinstance(node.func, ast.Name) and node.func.id in self.classes:
            return node.func.id
        return None

    def _update_exact_local(self, name: str, value: ast.expr | None) -> None:
        if isinstance(value, ast.Call):
            class_id = self._class_id_from_call(value)
            if class_id is not None:
                class_info = self.classes.get(class_id)
                if (
                    class_info
                    and not class_info.get("dynamic")
                    and not class_info.get("dataclass")
                ):
                    self.exact_locals[name] = class_id
                    return
        if isinstance(value, ast.Name):
            if value.id in self.exact_locals and (
                self.current_func_name == "molt_main"
                or value.id not in self.global_decls
            ):
                self.exact_locals[name] = self.exact_locals[value.id]
                return
        self.exact_locals.pop(name, None)

    def _load_local_value(self, name: str) -> MoltValue | None:
        if self.current_func_name != "molt_main" and name in self.global_decls:
            return self._emit_module_attr_get(name)
        if self.is_async():
            if name in self.async_locals:
                offset = self.async_locals[name]
                res = MoltValue(
                    self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
                )
                self.emit(
                    MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res)
                )
                return res
        if name in self.boxed_locals:
            cell = self.boxed_locals[name]
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            res = MoltValue(self.next_var())
            hint = self.boxed_local_hints.get(name)
            if hint is not None:
                res.type_hint = hint
            self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
            return res
        return self.locals.get(name)

    def _store_local_value(self, name: str, value: MoltValue) -> None:
        if self.current_func_name != "molt_main" and name in self.global_decls:
            self._emit_module_attr_set_runtime(name, value)
            return
        if name in self.boxed_locals:
            cell = self.boxed_locals[name]
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[cell, idx, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.boxed_local_hints[name] = value.type_hint
            return
        if self.is_async():
            if name not in self.async_locals:
                self._async_local_offset(name)
            offset = self.async_locals[name]
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, value],
                    result=MoltValue("none"),
                )
            )
            if value.type_hint:
                self.async_local_hints[name] = value.type_hint
            return
        self.locals[name] = value

    def _iterable_is_indexable(self, iterable: MoltValue | None) -> bool:
        if iterable is None:
            return False
        return iterable.type_hint in {
            "list",
            "tuple",
            "dict_keys_view",
            "dict_values_view",
            "dict_items_view",
            "range",
            "memoryview",
        }

    def _expr_may_yield(self, node: ast.AST) -> bool:
        if not self.is_async():
            return False

        class YieldVisitor(ast.NodeVisitor):
            def __init__(self) -> None:
                self.may_yield = False

            def visit_Await(self, node: ast.Await) -> None:
                self.may_yield = True

            def visit_Call(self, node: ast.Call) -> None:
                if isinstance(node.func, ast.Name) and node.func.id in {
                    "molt_chan_send",
                    "molt_chan_recv",
                }:
                    self.may_yield = True
                    return
                self.generic_visit(node)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

        visitor = YieldVisitor()
        visitor.visit(node)
        return visitor.may_yield

    def _spill_async_value(self, value: MoltValue, name: str) -> int:
        offset = self._async_local_offset(name)
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", offset, value],
                result=MoltValue("none"),
            )
        )
        return offset

    def _reload_async_value(self, offset: int, hint: str) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
        return res

    def _emit_call_args(self, args: list[ast.expr]) -> list[MoltValue]:
        if not args:
            return []
        if not self.is_async():
            values: list[MoltValue] = []
            for expr in args:
                arg = self.visit(expr)
                if arg is None:
                    raise NotImplementedError("Unsupported call argument")
                values.append(arg)
            return values
        yield_flags = [self._expr_may_yield(expr) for expr in args]
        if not any(yield_flags):
            values = []
            for expr in args:
                arg = self.visit(expr)
                if arg is None:
                    raise NotImplementedError("Unsupported call argument")
                values.append(arg)
            return values
        values = []
        spills: list[tuple[int, int, str]] = []
        for idx, expr in enumerate(args):
            arg = self.visit(expr)
            if arg is None:
                raise NotImplementedError("Unsupported call argument")
            values.append(arg)
            if any(yield_flags[idx + 1 :]):
                slot = self._spill_async_value(
                    arg, f"__arg_spill_{len(self.async_locals)}"
                )
                spills.append((idx, slot, arg.type_hint))
        for idx, slot, hint in spills:
            values[idx] = self._reload_async_value(slot, hint)
        return values

    def _match_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str] | None:
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        target_name = node.target.id
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if not isinstance(stmt.value, ast.Name):
                return None
            if stmt.value.id != target_name:
                return None
            if stmt.target.id == target_name:
                return None
            kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
            return (stmt.target.id, target_name, kind)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            dest = stmt.targets[0].id
            if dest == target_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == dest:
                if isinstance(right, ast.Name) and right.id == target_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, target_name, kind)
            if isinstance(right, ast.Name) and right.id == dest:
                if isinstance(left, ast.Name) and left.id == target_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, target_name, kind)
        return None

    def _range_start_expr(self, node: ast.expr) -> ast.expr | None:
        if isinstance(node, ast.Constant):
            if isinstance(node.value, int) and node.value > 0:
                return node
            return None
        if isinstance(node, ast.Name):
            return node
        return None

    def _match_indexed_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        idx_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Call):
            return None
        if not isinstance(node.iter.func, ast.Name) or node.iter.func.id != "range":
            return None
        args = node.iter.args
        if not args or len(args) > 3:
            return None
        start = None
        stop = None
        step = None
        if len(args) == 1:
            stop = args[0]
            step = ast.Constant(value=1)
        elif len(args) == 2:
            start = args[0]
            stop = args[1]
            step = ast.Constant(value=1)
        else:
            start = args[0]
            stop = args[1]
            step = args[2]
        start_expr = None
        if start is not None:
            if isinstance(start, ast.Constant):
                if not isinstance(start.value, int) or start.value < 0:
                    return None
                if start.value > 0:
                    start_expr = start
            else:
                start_expr = self._range_start_expr(start)
                if start_expr is None:
                    return None
        if not isinstance(step, ast.Constant) or step.value != 1:
            return None
        if not isinstance(stop, ast.Call):
            return None
        if not isinstance(stop.func, ast.Name) or stop.func.id != "len":
            return None
        if len(stop.args) != 1 or not isinstance(stop.args[0], ast.Name):
            return None
        seq_name = stop.args[0].id
        stmt = node.body[0]
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if stmt.target.id == idx_name:
                return None
            if not self._subscript_matches(stmt.value, seq_name, idx_name):
                return None
            kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
            return (stmt.target.id, seq_name, kind, start_expr)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            dest = stmt.targets[0].id
            if dest == idx_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == dest:
                if self._subscript_matches(right, seq_name, idx_name):
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, seq_name, kind, start_expr)
            if isinstance(right, ast.Name) and right.id == dest:
                if self._subscript_matches(left, seq_name, idx_name):
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (dest, seq_name, kind, start_expr)
        return None

    def _subscript_matches(self, node: ast.expr, seq_name: str, idx_name: str) -> bool:
        if not isinstance(node, ast.Subscript):
            return False
        if not isinstance(node.value, ast.Name) or node.value.id != seq_name:
            return False
        if isinstance(node.slice, ast.Name) and node.slice.id == idx_name:
            return True
        return False

    def _match_indexed_vector_minmax_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        idx_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Call):
            return None
        if not isinstance(node.iter.func, ast.Name) or node.iter.func.id != "range":
            return None
        args = node.iter.args
        if not args or len(args) > 3:
            return None
        start = None
        stop = None
        step = None
        if len(args) == 1:
            stop = args[0]
            step = ast.Constant(value=1)
        elif len(args) == 2:
            start = args[0]
            stop = args[1]
            step = ast.Constant(value=1)
        else:
            start = args[0]
            stop = args[1]
            step = args[2]
        start_expr = None
        if start is not None:
            if isinstance(start, ast.Constant):
                if not isinstance(start.value, int) or start.value < 0:
                    return None
                if start.value > 0:
                    start_expr = start
            else:
                start_expr = self._range_start_expr(start)
                if start_expr is None:
                    return None
        if not isinstance(step, ast.Constant) or step.value != 1:
            return None
        if not isinstance(stop, ast.Call):
            return None
        if not isinstance(stop.func, ast.Name) or stop.func.id != "len":
            return None
        if len(stop.args) != 1 or not isinstance(stop.args[0], ast.Name):
            return None
        seq_name = stop.args[0].id
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        if acc_name == idx_name:
            return None
        if not self._subscript_matches(assign.value, seq_name, idx_name):
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        left_is_acc = isinstance(left, ast.Name) and left.id == acc_name
        right_is_acc = isinstance(right, ast.Name) and right.id == acc_name
        left_is_item = self._subscript_matches(left, seq_name, idx_name)
        right_is_item = self._subscript_matches(right, seq_name, idx_name)
        if not ((left_is_acc and right_is_item) or (left_is_item and right_is_acc)):
            return None
        if isinstance(op, ast.Lt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "min", start_expr
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "max", start_expr
        if isinstance(op, ast.Gt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "max", start_expr
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "min", start_expr
        return None

    def _match_vector_minmax_loop(self, node: ast.For) -> tuple[str, str, str] | None:
        if not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        if not isinstance(stmt, ast.If) or stmt.orelse:
            return None
        if len(stmt.body) != 1:
            return None
        assign = stmt.body[0]
        if not isinstance(assign, ast.Assign):
            return None
        if len(assign.targets) != 1 or not isinstance(assign.targets[0], ast.Name):
            return None
        acc_name = assign.targets[0].id
        item_name = node.target.id
        if acc_name == item_name:
            return None
        if not isinstance(assign.value, ast.Name) or assign.value.id != item_name:
            return None
        test = stmt.test
        if not isinstance(test, ast.Compare):
            return None
        if len(test.ops) != 1 or len(test.comparators) != 1:
            return None
        op = test.ops[0]
        left = test.left
        right = test.comparators[0]
        if not isinstance(left, ast.Name) or not isinstance(right, ast.Name):
            return None
        if {left.id, right.id} != {item_name, acc_name}:
            return None
        if isinstance(op, ast.Lt):
            if left.id == item_name and right.id == acc_name:
                return acc_name, item_name, "min"
            if left.id == acc_name and right.id == item_name:
                return acc_name, item_name, "max"
        if isinstance(op, ast.Gt):
            if left.id == item_name and right.id == acc_name:
                return acc_name, item_name, "max"
            if left.id == acc_name and right.id == item_name:
                return acc_name, item_name, "min"
        return None

    def _emit_iter_loop(self, node: ast.For, iterable: MoltValue) -> None:
        target = node.target
        assert isinstance(target, ast.Name)
        if self.is_async():
            iter_obj = self._emit_iter_new(iterable)
            iter_slot = self._async_local_offset(f"__for_iter_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", iter_slot, iter_obj],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            iter_val = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", iter_slot],
                    result=iter_val,
                )
            )
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="ITER_NEXT", args=[iter_val], result=pair))
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_TRUE",
                    args=[done],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
            self._store_local_value(target.id, item)
            for stmt in node.body:
                self.visit(stmt)
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            return
        iter_obj = self._emit_iter_new(iterable)

        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))

        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self._store_local_value(target.id, item)
        for stmt in node.body:
            self.visit(stmt)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_index_loop(self, node: ast.For, iterable: MoltValue) -> None:
        target = node.target
        assert isinstance(target, ast.Name)
        if self.is_async():
            seq_slot = self._async_local_offset(f"__for_seq_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", seq_slot, iterable],
                    result=MoltValue("none"),
                )
            )
            length_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LEN", args=[iterable], result=length_val))
            length_slot = self._async_local_offset(
                f"__for_len_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", length_slot, length_val],
                    result=MoltValue("none"),
                )
            )
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            idx_slot = self._async_local_offset(f"__for_idx_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", idx_slot, zero],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", idx_slot],
                    result=idx,
                )
            )
            seq_val = MoltValue(self.next_var(), type_hint=iterable.type_hint)
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", seq_slot],
                    result=seq_val,
                )
            )
            length = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", length_slot],
                    result=length,
                )
            )
            cond = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[idx, length], result=cond))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[seq_val, idx], result=item))
            self._store_local_value(target.id, item)
            for stmt in node.body:
                self.visit(stmt)
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", idx_slot, next_idx],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            return
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        length = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LEN", args=[iterable], result=length))

        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[zero], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, length], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[iterable, idx], result=item))
        self._store_local_value(target.id, item)
        for stmt in node.body:
            self.visit(stmt)
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _parse_range_call(
        self, node: ast.AST
    ) -> tuple[MoltValue, MoltValue, MoltValue] | None:
        if not isinstance(node, ast.Call):
            return None
        if not isinstance(node.func, ast.Name) or node.func.id != "range":
            return None
        if len(node.args) == 1:
            start = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start))
            stop = self.visit(node.args[0])
            step = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step))
            return start, stop, step
        if len(node.args) == 2:
            start = self.visit(node.args[0])
            stop = self.visit(node.args[1])
            step = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step))
            return start, stop, step
        if len(node.args) == 3:
            start = self.visit(node.args[0])
            stop = self.visit(node.args[1])
            step = self.visit(node.args[2])
            return start, stop, step
        raise NotImplementedError("range expects 1, 2, or 3 arguments")

    def _emit_range_loop(
        self, node: ast.For, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> None:
        target = node.target
        assert isinstance(target, ast.Name)
        if self.is_async():
            range_obj = MoltValue(self.next_var(), type_hint="range")
            self.emit(
                MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=range_obj)
            )
            self._emit_iter_loop(node, range_obj)
            return None
        step_const = self.const_ints.get(step.name)
        if step_const is not None and step_const != 0:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
            cond = MoltValue(self.next_var(), type_hint="bool")
            if step_const > 0:
                self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
            else:
                self.emit(MoltOp(kind="LT", args=[stop, idx], result=cond))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none")
                )
            )
            self._store_local_value(target.id, idx)
            for stmt in node.body:
                self.visit(stmt)
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
            self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            return None
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        step_pos = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[zero, step], result=step_pos))
        self.emit(MoltOp(kind="IF", args=[step_pos], result=MoltValue("none")))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(target.id, idx)
        for stmt in node.body:
            self.visit(stmt)
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        step_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[step, zero], result=step_neg))
        self.emit(MoltOp(kind="IF", args=[step_neg], result=MoltValue("none")))
        idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx_neg))
        cond_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[stop, idx_neg], result=cond_neg))
        self.emit(
            MoltOp(
                kind="LOOP_BREAK_IF_FALSE",
                args=[cond_neg],
                result=MoltValue("none"),
            )
        )
        self._store_local_value(target.id, idx_neg)
        for stmt in node.body:
            self.visit(stmt)
        next_idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx_neg, step], result=next_idx_neg))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx_neg], result=idx_neg))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_range_list(
        self, start: MoltValue, stop: MoltValue, step: MoltValue
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        step_const = self.const_ints.get(step.name)
        if step_const is not None and step_const != 0:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
            cond = MoltValue(self.next_var(), type_hint="bool")
            if step_const > 0:
                self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
            else:
                self.emit(MoltOp(kind="LT", args=[stop, idx], result=cond))
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none")
                )
            )
            self.emit(
                MoltOp(kind="LIST_APPEND", args=[res, idx], result=MoltValue("none"))
            )
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
            self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            return res
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        step_pos = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[zero, step], result=step_pos))
        self.emit(MoltOp(kind="IF", args=[step_pos], result=MoltValue("none")))

        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LIST_APPEND", args=[res, idx], result=MoltValue("none")))
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        step_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[step, zero], result=step_neg))
        self.emit(MoltOp(kind="IF", args=[step_neg], result=MoltValue("none")))
        idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx_neg))
        cond_neg = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[stop, idx_neg], result=cond_neg))
        self.emit(
            MoltOp(
                kind="LOOP_BREAK_IF_FALSE",
                args=[cond_neg],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(kind="LIST_APPEND", args=[res, idx_neg], result=MoltValue("none"))
        )
        next_idx_neg = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx_neg, step], result=next_idx_neg))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx_neg], result=idx_neg))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_list_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        iter_obj = self._emit_iter_new(iterable)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(
            MoltOp(kind="LIST_APPEND", args=[res, item], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _emit_tuple_from_iter(self, iterable: MoltValue) -> MoltValue:
        items = self._emit_list_from_iter(iterable)
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_FROM_LIST", args=[items], result=res))
        return res

    def _emit_intarray_from_seq(self, seq: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="intarray")
        self.emit(MoltOp(kind="INTARRAY_FROM_SEQ", args=[seq], result=res))
        self.container_elem_hints[res.name] = "int"
        return res

    def _emit_iter_new(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=res))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[res, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        err_val = self._emit_exception_new("TypeError", "object is not iterable")
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_guarded_setattr(
        self, obj: MoltValue, attr: str, value: MoltValue, expected_class: str
    ) -> None:
        class_ref = self._emit_module_attr_get(expected_class)
        obj_type = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="TYPE_OF", args=[obj], result=obj_type))
        matches = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[obj_type, class_ref], result=matches))
        self.emit(MoltOp(kind="IF", args=[matches], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR",
                args=[obj, attr, value, expected_class],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[obj, attr, value],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_aiter(self, iterable: MoltValue) -> MoltValue:
        if iterable.type_hint in {
            "list",
            "tuple",
            "dict",
            "range",
            "iter",
            "generator",
        }:
            return self._emit_iter_new(iterable)
        res = MoltValue(self.next_var(), type_hint="async_iter")
        self.emit(MoltOp(kind="AITER", args=[iterable], result=res))
        return res

    def _emit_for_loop(self, node: ast.For, iterable: MoltValue) -> None:
        if self._iterable_is_indexable(iterable):
            self._emit_index_loop(node, iterable)
        else:
            self._emit_iter_loop(node, iterable)

    def _match_counted_while(
        self, node: ast.While
    ) -> tuple[str, int, list[ast.stmt]] | None:
        if node.orelse:
            return None
        if not isinstance(node.test, ast.Compare):
            return None
        if len(node.test.ops) != 1 or not isinstance(node.test.ops[0], ast.Lt):
            return None
        if not isinstance(node.test.left, ast.Name):
            return None
        if len(node.test.comparators) != 1:
            return None
        bound = node.test.comparators[0]
        if not (isinstance(bound, ast.Constant) and isinstance(bound.value, int)):
            return None
        if not node.body:
            return None
        index_name = node.test.left.id
        incr_stmt = node.body[-1]
        if not self._is_unit_increment(incr_stmt, index_name):
            return None
        if index_name in self._collect_assigned_names(node.body[:-1]):
            return None
        return index_name, bound.value, node.body[:-1]

    def _match_counted_while_sum(
        self, index_name: str, body: list[ast.stmt]
    ) -> str | None:
        if len(body) != 1:
            return None
        stmt = body[0]
        if isinstance(stmt, ast.AugAssign):
            if (
                isinstance(stmt.op, ast.Add)
                and isinstance(stmt.target, ast.Name)
                and isinstance(stmt.value, ast.Name)
                and stmt.value.id == index_name
            ):
                return stmt.target.id
            return None
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and isinstance(right, ast.Name)
                and (
                    {left.id, right.id} == {acc_name, index_name}
                    and left.id != right.id
                )
            ):
                return acc_name
        return None

    def _is_unit_increment(self, stmt: ast.stmt, name: str) -> bool:
        if isinstance(stmt, ast.AugAssign):
            if isinstance(stmt.target, ast.Name) and stmt.target.id == name:
                return (
                    isinstance(stmt.op, ast.Add)
                    and isinstance(stmt.value, ast.Constant)
                    and stmt.value.value == 1
                )
            return False
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return False
            if stmt.targets[0].id != name:
                return False
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, ast.Add
            ):
                return False
            left = stmt.value.left
            right = stmt.value.right
            if (
                isinstance(left, ast.Name)
                and left.id == name
                and isinstance(right, ast.Constant)
                and right.value == 1
            ):
                return True
            if (
                isinstance(right, ast.Name)
                and right.id == name
                and isinstance(left, ast.Constant)
                and left.value == 1
            ):
                return True
        return False

    def _emit_counted_while(
        self, index_name: str, bound: int, body: list[ast.stmt]
    ) -> None:
        start = self._load_local_value(index_name)
        if start is None:
            start = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=start))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        stop = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[bound], result=stop))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(index_name, idx)
        for stmt in body:
            self.visit(stmt)
        next_idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
        self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def visit_BinOp(self, node: ast.BinOp) -> Any:
        left = self.visit(node.left)
        if left is None:
            raise NotImplementedError("Unsupported binary operator left operand")
        left_slot: int | None = None
        if self.is_async() and self._expr_may_yield(node.right):
            left_slot = self._spill_async_value(
                left, f"__binop_left_{len(self.async_locals)}"
            )
        right = self.visit(node.right)
        if right is None:
            raise NotImplementedError("Unsupported binary operator right operand")
        if left_slot is not None:
            left = self._reload_async_value(left_slot, left.type_hint)
        res_type = "Unknown"
        hint_src: MoltValue | None = None
        if isinstance(node.op, ast.Add):
            op_kind = "ADD"
            if left.type_hint == right.type_hint and left.type_hint in {
                "int",
                "float",
                "str",
                "bytes",
                "bytearray",
                "list",
                "tuple",
            }:
                res_type = left.type_hint
            elif {left.type_hint, right.type_hint} == {"int", "float"}:
                res_type = "float"
        elif isinstance(node.op, ast.Sub):
            op_kind = "SUB"
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.Mult):
            op_kind = "MUL"
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
            elif left.type_hint in {"list", "tuple"} and right.type_hint == "int":
                res_type = left.type_hint
                hint_src = left
            elif right.type_hint in {"list", "tuple"} and left.type_hint == "int":
                res_type = right.type_hint
                hint_src = right
        else:
            op_kind = "UNKNOWN"
        res = MoltValue(self.next_var(), type_hint=res_type)
        self.emit(MoltOp(kind=op_kind, args=[left, right], result=res))
        if hint_src is not None:
            self._propagate_container_hints(res.name, hint_src)
        return res

    def visit_Constant(self, node: ast.Constant) -> Any:
        if node.value is None:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            return res
        if isinstance(node.value, bool):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[node.value], result=res))
            return res
        if isinstance(node.value, bytes):
            res = MoltValue(self.next_var(), type_hint="bytes")
            self.emit(MoltOp(kind="CONST_BYTES", args=[node.value], result=res))
            return res
        if isinstance(node.value, float):
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[node.value], result=res))
            return res
        if isinstance(node.value, str):
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.value], result=res))
            return res
        res = MoltValue(self.next_var(), type_hint=type(node.value).__name__)
        self.emit(MoltOp(kind="CONST", args=[node.value], result=res))
        return res

    def _emit_str_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STR_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_string_join(self, parts: list[MoltValue]) -> MoltValue:
        if not parts:
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
            return res
        if len(parts) == 1:
            return parts[0]
        sep = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=sep))
        items = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=parts, result=items))
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_JOIN", args=[sep, items], result=res))
        return res

    def _emit_string_format(self, value: MoltValue, spec: str) -> MoltValue:
        spec_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[spec], result=spec_val))
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_FORMAT", args=[value, spec_val], result=res))
        return res

    def _emit_not(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[value], result=res))
        return res

    def _emit_contains(self, container: MoltValue, item: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONTAINS", args=[container, item], result=res))
        return res

    def _emit_compare_op(
        self, op: ast.cmpop, left: MoltValue, right: MoltValue
    ) -> MoltValue:
        if isinstance(op, ast.Eq):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="EQ", args=[left, right], result=res))
            return res
        if isinstance(op, ast.NotEq):
            eq_val = self._emit_compare_op(ast.Eq(), left, right)
            return self._emit_not(eq_val)
        if isinstance(op, ast.Lt):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Gt):
            return self._emit_compare_op(ast.Lt(), right, left)
        if isinstance(op, ast.LtE):
            lt_val = self._emit_compare_op(ast.Lt(), right, left)
            return self._emit_not(lt_val)
        if isinstance(op, ast.GtE):
            lt_val = self._emit_compare_op(ast.Lt(), left, right)
            return self._emit_not(lt_val)
        if isinstance(op, ast.Is):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[left, right], result=res))
            return res
        if isinstance(op, ast.IsNot):
            is_val = self._emit_compare_op(ast.Is(), left, right)
            return self._emit_not(is_val)
        if isinstance(op, ast.In):
            return self._emit_contains(right, left)
        if isinstance(op, ast.NotIn):
            in_val = self._emit_contains(right, left)
            return self._emit_not(in_val)
        raise NotImplementedError("Comparison operator not supported")

    def _format_spec_to_str(self, node: ast.expr) -> str:
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.JoinedStr):
            parts: list[str] = []
            for item in node.values:
                if isinstance(item, ast.Constant) and isinstance(item.value, str):
                    parts.append(item.value)
                else:
                    raise NotImplementedError(
                        "Dynamic f-string format specs are not supported"
                    )
            return "".join(parts)
        raise NotImplementedError("Unsupported f-string format spec")

    def _parse_format_literal(self, text: str) -> list[tuple[str, str | int, str]]:
        parts: list[tuple[str, str | int, str]] = []
        idx = 0
        implicit = 0
        auto_used = False
        manual_used = False
        while idx < len(text):
            ch = text[idx]
            if ch == "{":
                if idx + 1 < len(text) and text[idx + 1] == "{":
                    parts.append(("text", "{", ""))
                    idx += 2
                    continue
                end = text.find("}", idx + 1)
                if end == -1:
                    raise NotImplementedError("Unclosed format placeholder")
                inner = text[idx + 1 : end]
                if "!" in inner:
                    raise NotImplementedError(
                        "Format conversion flags are not supported"
                    )
                if ":" in inner:
                    field, spec = inner.split(":", 1)
                else:
                    field, spec = inner, ""
                if field == "":
                    auto_used = True
                    if manual_used:
                        raise NotImplementedError(
                            "Cannot mix automatic and manual field numbering"
                        )
                    parts.append(("arg", implicit, spec))
                    implicit += 1
                elif field.isdigit():
                    manual_used = True
                    if auto_used:
                        raise NotImplementedError(
                            "Cannot mix automatic and manual field numbering"
                        )
                    parts.append(("arg", int(field), spec))
                else:
                    if "." in field or "[" in field:
                        raise NotImplementedError(
                            "Format field access is not supported"
                        )
                    if not (field[0].isalpha() or field[0] == "_"):
                        raise NotImplementedError("Invalid format field name")
                    if not field.replace("_", "").isalnum():
                        raise NotImplementedError("Invalid format field name")
                    manual_used = True
                    if auto_used:
                        raise NotImplementedError(
                            "Cannot mix automatic and manual field numbering"
                        )
                    parts.append(("arg", field, spec))
                idx = end + 1
                continue
            if ch == "}":
                if idx + 1 < len(text) and text[idx + 1] == "}":
                    parts.append(("text", "}", ""))
                    idx += 2
                    continue
                raise NotImplementedError("Single '}' in format string")
            start = idx
            while idx < len(text) and text[idx] not in "{}":
                idx += 1
            parts.append(("text", text[start:idx], ""))
        return parts

    def _parse_molt_buffer_call(
        self, node: ast.Call, name: str
    ) -> list[ast.expr] | None:
        if (
            isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "molt_buffer"
            and node.func.attr == name
        ):
            return node.args
        return None

    def _match_matmul_loop(self, node: ast.For) -> tuple[str, str, str] | None:
        if node.orelse or not isinstance(node.target, ast.Name):
            return None
        if len(node.body) != 1 or not isinstance(node.body[0], ast.For):
            return None
        outer_i = node.target.id
        j_loop = node.body[0]
        if j_loop.orelse or not isinstance(j_loop.target, ast.Name):
            return None
        inner_j = j_loop.target.id
        if len(j_loop.body) != 3:
            return None
        init = j_loop.body[0]
        k_loop = j_loop.body[1]
        store = j_loop.body[2]
        if not isinstance(init, ast.Assign):
            return None
        if len(init.targets) != 1 or not isinstance(init.targets[0], ast.Name):
            return None
        acc_name = init.targets[0].id
        if not isinstance(init.value, ast.Constant) or init.value.value != 0:
            return None
        if not isinstance(k_loop, ast.For) or k_loop.orelse:
            return None
        if not isinstance(k_loop.target, ast.Name):
            return None
        inner_k = k_loop.target.id
        if len(k_loop.body) != 1 or not isinstance(k_loop.body[0], ast.Assign):
            return None
        acc_assign = k_loop.body[0]
        if (
            len(acc_assign.targets) != 1
            or not isinstance(acc_assign.targets[0], ast.Name)
            or acc_assign.targets[0].id != acc_name
        ):
            return None
        if not isinstance(acc_assign.value, ast.BinOp) or not isinstance(
            acc_assign.value.op, ast.Add
        ):
            return None
        add_left = acc_assign.value.left
        add_right = acc_assign.value.right
        if not isinstance(add_left, ast.Name) or add_left.id != acc_name:
            return None
        if not isinstance(add_right, ast.BinOp) or not isinstance(
            add_right.op, ast.Mult
        ):
            return None
        left_get = add_right.left
        right_get = add_right.right
        if not (isinstance(left_get, ast.Call) and isinstance(right_get, ast.Call)):
            return None
        left_args = self._parse_molt_buffer_call(left_get, "get")
        right_args = self._parse_molt_buffer_call(right_get, "get")
        if left_args is None or right_args is None:
            return None
        if len(left_args) != 3 or len(right_args) != 3:
            return None
        if not all(isinstance(arg, ast.Name) for arg in left_args[1:]):
            return None
        if not all(isinstance(arg, ast.Name) for arg in right_args[1:]):
            return None
        left_buf = left_args[0]
        right_buf = right_args[0]
        if not isinstance(left_buf, ast.Name) or not isinstance(right_buf, ast.Name):
            return None
        a_name = left_buf.id
        b_name = right_buf.id
        left_i = cast(ast.Name, left_args[1]).id
        left_k = cast(ast.Name, left_args[2]).id
        right_k = cast(ast.Name, right_args[1]).id
        right_j = cast(ast.Name, right_args[2]).id
        if left_i != outer_i or left_k != inner_k:
            return None
        if right_k != inner_k or right_j != inner_j:
            return None
        if not isinstance(store, ast.Expr) or not isinstance(store.value, ast.Call):
            return None
        store_args = self._parse_molt_buffer_call(store.value, "set")
        if store_args is None or len(store_args) != 4:
            return None
        if not isinstance(store_args[0], ast.Name):
            return None
        out_name = store_args[0].id
        if not all(isinstance(arg, ast.Name) for arg in store_args[1:3]):
            return None
        if (
            cast(ast.Name, store_args[1]).id != outer_i
            or cast(ast.Name, store_args[2]).id != inner_j
        ):
            return None
        if not isinstance(store_args[3], ast.Name) or store_args[3].id != acc_name:
            return None
        return out_name, a_name, b_name

    def visit_JoinedStr(self, node: ast.JoinedStr) -> Any:
        parts: list[MoltValue] = []
        for item in node.values:
            if isinstance(item, ast.Constant) and isinstance(item.value, str):
                lit = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[item.value], result=lit))
                parts.append(lit)
                continue
            if isinstance(item, ast.FormattedValue):
                if item.conversion != -1:
                    raise NotImplementedError(
                        "Formatted value conversion not supported"
                    )
                value = self.visit(item.value)
                if item.format_spec is None:
                    parts.append(self._emit_str_from_obj(value))
                    continue
                spec_text = self._format_spec_to_str(item.format_spec)
                parts.append(self._emit_string_format(value, spec_text))
                continue
            raise NotImplementedError("Unsupported f-string segment")
        return self._emit_string_join(parts)

    def visit_List(self, node: ast.List) -> Any:
        elems = [self.visit(elt) for elt in node.elts]
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=elems, result=res))
        if elems:
            first = elems[0].type_hint
            if first in {"int", "float", "str", "bytes", "bytearray", "bool"} and all(
                elem.type_hint == first for elem in elems
            ):
                if self.current_func_name == "molt_main":
                    self.global_elem_hints[res.name] = first
                else:
                    self.container_elem_hints[res.name] = first
        return res

    def visit_Tuple(self, node: ast.Tuple) -> Any:
        elems = [self.visit(elt) for elt in node.elts]
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=elems, result=res))
        if elems:
            first = elems[0].type_hint
            if first in {"int", "float", "str", "bytes", "bytearray", "bool"} and all(
                elem.type_hint == first for elem in elems
            ):
                if self.current_func_name == "molt_main":
                    self.global_elem_hints[res.name] = first
                else:
                    self.container_elem_hints[res.name] = first
        return res

    def visit_Dict(self, node: ast.Dict) -> Any:
        items: list[MoltValue] = []
        for key, value in zip(node.keys, node.values):
            if key is None:
                raise NotImplementedError("Dict unpacking is not supported")
            items.append(self.visit(key))
            items.append(self.visit(value))
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=items, result=res))
        if items:
            key_vals = items[::2]
            val_vals = items[1::2]
            if all(key.type_hint == "str" for key in key_vals):
                first_val = val_vals[0].type_hint
                if first_val in {
                    "int",
                    "float",
                    "str",
                    "bytes",
                    "bytearray",
                    "bool",
                } and all(val.type_hint == first_val for val in val_vals):
                    if self.current_func_name == "molt_main":
                        self.global_dict_key_hints[res.name] = "str"
                        self.global_dict_value_hints[res.name] = first_val
                    else:
                        self.dict_key_hints[res.name] = "str"
                        self.dict_value_hints[res.name] = first_val
        return res

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        base_vals: list[MoltValue] = []
        base_names: list[str] = []
        if node.bases:
            if node.keywords:
                raise NotImplementedError("Class keywords are not supported")
            for base_expr in node.bases:
                if not isinstance(base_expr, ast.Name):
                    raise NotImplementedError("Unsupported base class expression")
                base_val = self.visit(base_expr)
                if base_val is None:
                    raise NotImplementedError("Base class must be defined before use")
                base_vals.append(base_val)
                base_names.append(base_expr.id)
        elif node.keywords:
            raise NotImplementedError("Class keywords are not supported")

        if not base_vals:
            tag_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[BUILTIN_TYPE_TAGS["object"]], result=tag_val)
            )
            base_val = MoltValue(self.next_var(), type_hint="type")
            self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=base_val))
            base_vals = [base_val]
            base_names = ["object"]

        dataclass_opts = None
        if node.decorator_list:
            for deco in node.decorator_list:
                if isinstance(deco, ast.Name) and deco.id == "dataclass":
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
                    dataclass_opts = {
                        "frozen": False,
                        "eq": True,
                        "repr": True,
                        "slots": False,
                    }
                    continue
                if (
                    isinstance(deco, ast.Attribute)
                    and isinstance(deco.value, ast.Name)
                    and deco.value.id == "dataclasses"
                    and deco.attr == "dataclass"
                ):
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
                    dataclass_opts = {
                        "frozen": False,
                        "eq": True,
                        "repr": True,
                        "slots": False,
                    }
                    continue
                if (
                    isinstance(deco, ast.Call)
                    and isinstance(deco.func, ast.Name)
                    and deco.func.id == "dataclass"
                ):
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
                    dataclass_opts = {
                        "frozen": False,
                        "eq": True,
                        "repr": True,
                        "slots": False,
                    }
                    for kw in deco.keywords:
                        if kw.arg not in {"frozen", "eq", "repr", "slots"}:
                            raise NotImplementedError(
                                f"Unsupported dataclass option: {kw.arg}"
                            )
                        if not isinstance(kw.value, ast.Constant) or not isinstance(
                            kw.value.value, bool
                        ):
                            raise NotImplementedError(
                                f"dataclass {kw.arg} must be a boolean literal"
                            )
                        dataclass_opts[kw.arg] = kw.value.value
                    continue
                if (
                    isinstance(deco, ast.Call)
                    and isinstance(deco.func, ast.Attribute)
                    and isinstance(deco.func.value, ast.Name)
                    and deco.func.value.id == "dataclasses"
                    and deco.func.attr == "dataclass"
                ):
                    if dataclass_opts is not None:
                        raise NotImplementedError(
                            "Multiple dataclass decorators are not supported"
                        )
                    dataclass_opts = {
                        "frozen": False,
                        "eq": True,
                        "repr": True,
                        "slots": False,
                    }
                    for kw in deco.keywords:
                        if kw.arg not in {"frozen", "eq", "repr", "slots"}:
                            raise NotImplementedError(
                                f"Unsupported dataclass option: {kw.arg}"
                            )
                        if not isinstance(kw.value, ast.Constant) or not isinstance(
                            kw.value.value, bool
                        ):
                            raise NotImplementedError(
                                f"dataclass {kw.arg} must be a boolean literal"
                            )
                        dataclass_opts[kw.arg] = kw.value.value
                    continue
                raise NotImplementedError("Unsupported class decorator")

        methods: dict[str, MethodInfo] = {}
        class_attrs: dict[str, ast.expr] = {}
        if len(base_names) != len(set(base_names)):
            dup = next(name for name in base_names if base_names.count(name) > 1)
            raise NotImplementedError(f"Duplicate base class {dup}")

        dynamic = len(base_names) > 1
        for name in base_names:
            base_info = self.classes.get(name)
            if base_info and base_info.get("dynamic"):
                dynamic = True

        base_mros = [self._class_mro_names(name) for name in base_names]
        base_mros.append(list(base_names))
        merged = self._c3_merge(base_mros)
        if merged is None:
            raise NotImplementedError(
                "Cannot create a consistent method resolution order (MRO) for bases"
            )
        mro_names = [node.name] + merged

        if dataclass_opts is not None:
            if any(name != "object" for name in base_names):
                raise NotImplementedError("Dataclass inheritance is not supported")
            field_order: list[str] = []
            field_defaults: dict[str, ast.expr] = {}
            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    name = item.target.id
                    field_order.append(name)
                    if item.value is not None:
                        field_defaults[name] = item.value
                        class_attrs[name] = item.value
                if isinstance(item, ast.Assign):
                    for target in item.targets:
                        if isinstance(target, ast.Name):
                            class_attrs[target.id] = item.value
            field_indices = {name: idx for idx, name in enumerate(field_order)}
            self.classes[node.name] = {
                "fields": field_indices,
                "field_order": field_order,
                "defaults": field_defaults,
                "class_attrs": class_attrs,
                "bases": base_names,
                "mro": mro_names,
                "dynamic": False,
                "size": len(field_order) * 8,
                "dataclass": True,
                "frozen": dataclass_opts["frozen"],
                "eq": dataclass_opts["eq"],
                "repr": dataclass_opts["repr"],
                "slots": dataclass_opts["slots"],
                "methods": methods,
            }
        else:
            fields: dict[str, int] = {}
            field_order: list[str] = []
            field_defaults: dict[str, ast.expr] = {}
            for base_name in mro_names[1:]:
                base_info = self.classes.get(base_name)
                if base_info is None:
                    continue
                for field in base_info.get("field_order", []):
                    if field not in fields:
                        fields[field] = len(field_order) * 8
                        field_order.append(field)
                for name, expr in base_info.get("defaults", {}).items():
                    if name not in field_defaults:
                        field_defaults[name] = expr

            def add_field(name: str) -> None:
                if name in fields:
                    return
                fields[name] = len(field_order) * 8
                field_order.append(name)

            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    add_field(item.target.id)
                    if item.value is not None:
                        field_defaults[item.target.id] = item.value
                        class_attrs[item.target.id] = item.value
                if isinstance(item, ast.Assign):
                    for target in item.targets:
                        if isinstance(target, ast.Name):
                            class_attrs[target.id] = item.value

            methods_in_body = [
                item for item in node.body if isinstance(item, ast.FunctionDef)
            ]

            if methods_in_body:

                class FieldCollector(ast.NodeVisitor):
                    def __init__(self, add: Callable[[str], None]) -> None:
                        self._add = add

                    def visit_Assign(self, node: ast.Assign) -> None:
                        for target in node.targets:
                            self._handle_target(target)
                        self.generic_visit(node.value)

                    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                        self._handle_target(node.target)
                        if node.value is not None:
                            self.generic_visit(node.value)

                    def _handle_target(self, target: ast.AST) -> None:
                        if (
                            isinstance(target, ast.Attribute)
                            and isinstance(target.value, ast.Name)
                            and target.value.id == "self"
                        ):
                            self._add(target.attr)

                    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                        return

                    def visit_AsyncFunctionDef(
                        self, node: ast.AsyncFunctionDef
                    ) -> None:
                        return

                    def visit_Lambda(self, node: ast.Lambda) -> None:
                        return

                collector = FieldCollector(add_field)
                for method in methods_in_body:
                    for stmt in method.body:
                        collector.visit(stmt)

            self.classes[node.name] = ClassInfo(
                fields=fields,
                size=(len(field_order) * 8 + 8) if not dynamic else 8,
                methods=methods,
                field_order=field_order,
                defaults=field_defaults,
                class_attrs=class_attrs,
                bases=base_names,
                mro=mro_names,
                dynamic=dynamic,
            )

        def compile_method(item: ast.FunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function", "classmethod", "staticmethod", "property"
            ] = "function"
            if item.decorator_list:
                if len(item.decorator_list) > 1:
                    raise NotImplementedError(
                        "Multiple method decorators not supported"
                    )
                deco = item.decorator_list[0]
                if isinstance(deco, ast.Name) and deco.id in {
                    "classmethod",
                    "staticmethod",
                    "property",
                }:
                    descriptor = cast(
                        Literal["function", "classmethod", "staticmethod", "property"],
                        deco.id,
                    )
                else:
                    raise NotImplementedError("Unsupported method decorator")
            method_name = item.name
            return_hint = self._annotation_to_hint(item.returns)
            if (
                return_hint
                and return_hint[:1] in {"'", '"'}
                and return_hint[-1:] == return_hint[:1]
            ):
                return_hint = return_hint[1:-1]
            if return_hint == "Self":
                return_hint = node.name
            method_symbol = self._function_symbol(f"{node.name}_{method_name}")
            params = [arg.arg for arg in item.args.args]
            has_return = self._function_contains_return(item)
            func_val = MoltValue(self.next_var(), type_hint=f"Func:{method_symbol}")
            self.emit(
                MoltOp(
                    kind="FUNC_NEW", args=[method_symbol, len(params)], result=func_val
                )
            )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=f"{node.name}.{method_name}",
                params=params,
                default_exprs=item.args.defaults,
                docstring=ast.get_docstring(item),
            )

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = node.name
            self.current_method_first_param = params[0] if params else None
            self.start_function(
                method_symbol,
                params=params,
                type_facts_name=f"{node.name}.{method_name}",
                needs_return_slot=has_return,
            )
            for idx, arg in enumerate(item.args.args):
                hint = None
                if idx == 0 and descriptor == "classmethod":
                    hint = node.name
                elif idx == 0 and arg.arg == "self":
                    hint = node.name
                if self.type_hint_policy in {"trust", "check"}:
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "int")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in item.args.args:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            for stmt in item.body:
                self.visit(stmt)
            if self.return_label is not None:
                if not self._ends_with_return_jump():
                    res = MoltValue(self.next_var())
                    self.emit(MoltOp(kind="CONST", args=[0], result=res))
                    self._emit_return_value(res)
                self._emit_return_label()
            elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
                res = MoltValue(self.next_var())
                self.emit(MoltOp(kind="CONST", args=[0], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param
            method_attr = func_val
            if descriptor == "classmethod":
                wrapped = MoltValue(self.next_var(), type_hint="classmethod")
                self.emit(
                    MoltOp(kind="CLASSMETHOD_NEW", args=[func_val], result=wrapped)
                )
                method_attr = wrapped
            elif descriptor == "staticmethod":
                wrapped = MoltValue(self.next_var(), type_hint="staticmethod")
                self.emit(
                    MoltOp(kind="STATICMETHOD_NEW", args=[func_val], result=wrapped)
                )
                method_attr = wrapped
            elif descriptor == "property":
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                wrapped = MoltValue(self.next_var(), type_hint="property")
                self.emit(
                    MoltOp(
                        kind="PROPERTY_NEW",
                        args=[func_val, none_val, none_val],
                        result=wrapped,
                    )
                )
                method_attr = wrapped
            return {
                "func": func_val,
                "attr": method_attr,
                "descriptor": descriptor,
                "return_hint": return_hint,
            }

        def compile_async_method(item: ast.AsyncFunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function", "classmethod", "staticmethod", "property"
            ] = "function"
            if item.decorator_list:
                if len(item.decorator_list) > 1:
                    raise NotImplementedError(
                        "Multiple method decorators not supported"
                    )
                deco = item.decorator_list[0]
                if isinstance(deco, ast.Name) and deco.id in {
                    "classmethod",
                    "staticmethod",
                    "property",
                }:
                    descriptor = cast(
                        Literal["function", "classmethod", "staticmethod", "property"],
                        deco.id,
                    )
                else:
                    raise NotImplementedError("Unsupported method decorator")
            method_name = item.name
            return_hint = self._annotation_to_hint(item.returns)
            if (
                return_hint
                and return_hint[:1] in {"'", '"'}
                and return_hint[-1:] == return_hint[:1]
            ):
                return_hint = return_hint[1:-1]
            if return_hint == "Self":
                return_hint = node.name
            wrapper_symbol = self._function_symbol(f"{node.name}_{method_name}")
            poll_symbol = f"{wrapper_symbol}_poll"
            params = [arg.arg for arg in item.args.args]
            has_return = self._function_contains_return(item)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = node.name
            self.current_method_first_param = params[0] if params else None
            self.start_function(
                poll_symbol,
                params=["self"],
                type_facts_name=f"{node.name}.{method_name}",
                needs_return_slot=has_return,
            )
            for i, arg in enumerate(item.args.args):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                hint = None
                if i == 0 and descriptor == "classmethod":
                    hint = node.name
                elif i == 0 and arg.arg == "self":
                    hint = node.name
                if self.type_hint_policy in {"trust", "check"}:
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                if hint is not None:
                    self.async_local_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            if self.type_hint_policy == "check":
                for arg in item.args.args:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            for stmt in item.body:
                self.visit(stmt)
            if self.return_label is not None:
                if not self._ends_with_return_jump():
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    self._emit_return_value(res)
                self._emit_return_label()
            else:
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            closure_size = self.async_locals_base + len(self.async_locals) * 8
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param

            func_val = MoltValue(self.next_var(), type_hint=f"Func:{wrapper_symbol}")
            self.emit(
                MoltOp(
                    kind="FUNC_NEW",
                    args=[wrapper_symbol, len(params)],
                    result=func_val,
                )
            )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=f"{node.name}.{method_name}",
                params=params,
                default_exprs=item.args.defaults,
                docstring=ast.get_docstring(item),
                is_coroutine=True,
            )

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            self.start_function(
                wrapper_symbol,
                params=params,
                type_facts_name=f"{node.name}.{method_name}",
            )
            for idx, arg in enumerate(item.args.args):
                hint = None
                if idx == 0 and descriptor == "classmethod":
                    hint = node.name
                elif idx == 0 and arg.arg == "self":
                    hint = node.name
                if self.type_hint_policy in {"trust", "check"}:
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "int")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in item.args.args:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            args = [self.locals[arg.arg] for arg in item.args.args]
            res = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="ALLOC_FUTURE",
                    args=[poll_symbol, closure_size] + args,
                    result=res,
                )
            )
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)

            method_attr = func_val
            if descriptor == "classmethod":
                wrapped = MoltValue(self.next_var(), type_hint="classmethod")
                self.emit(
                    MoltOp(kind="CLASSMETHOD_NEW", args=[func_val], result=wrapped)
                )
                method_attr = wrapped
            elif descriptor == "staticmethod":
                wrapped = MoltValue(self.next_var(), type_hint="staticmethod")
                self.emit(
                    MoltOp(kind="STATICMETHOD_NEW", args=[func_val], result=wrapped)
                )
                method_attr = wrapped
            elif descriptor == "property":
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                wrapped = MoltValue(self.next_var(), type_hint="property")
                self.emit(
                    MoltOp(
                        kind="PROPERTY_NEW",
                        args=[func_val, none_val, none_val],
                        result=wrapped,
                    )
                )
                method_attr = wrapped
            return {
                "func": func_val,
                "attr": method_attr,
                "descriptor": descriptor,
                "return_hint": return_hint,
            }

        for item in node.body:
            if isinstance(item, ast.FunctionDef):
                methods[item.name] = compile_method(item)
            elif isinstance(item, ast.AsyncFunctionDef):
                methods[item.name] = compile_async_method(item)

        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[node.name], result=name_val))
        class_val = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="CLASS_NEW", args=[name_val], result=class_val))
        if base_vals:
            if len(base_vals) == 1:
                bases_arg = base_vals[0]
            else:
                bases_arg = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=base_vals, result=bases_arg))
            self.emit(
                MoltOp(
                    kind="CLASS_SET_BASE",
                    args=[class_val, bases_arg],
                    result=MoltValue("none"),
                )
            )
        self.globals[node.name] = class_val
        self._emit_module_attr_set(node.name, class_val)

        class_info = self.classes[node.name]
        if (
            not class_info.get("dataclass")
            and not class_info.get("dynamic")
            and class_info.get("fields")
        ):
            field_items: list[MoltValue] = []
            for field in sorted(class_info["fields"]):
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[field], result=key_val))
                offset_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST",
                        args=[class_info["fields"][field]],
                        result=offset_val,
                    )
                )
                field_items.extend([key_val, offset_val])
            offsets_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=field_items, result=offsets_dict))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[class_val, "__molt_field_offsets__", offsets_dict],
                    result=MoltValue("none"),
                )
            )

        for attr_name, expr in class_attrs.items():
            val = self.visit(expr)
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[class_val, attr_name, val],
                    result=MoltValue("none"),
                )
            )

        for method_name, method_info in methods.items():
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[class_val, method_name, method_info["attr"]],
                    result=MoltValue("none"),
                )
            )

        self.emit(
            MoltOp(
                kind="CLASS_APPLY_SET_NAME",
                args=[class_val],
                result=MoltValue("none"),
            )
        )

        return None

    def visit_Call(self, node: ast.Call) -> Any:
        if isinstance(node.func, ast.Attribute):
            # ...
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "contextlib"
                and node.func.attr == "nullcontext"
            ):
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "contextlib"
                and node.func.attr == "closing"
            ):
                if len(node.args) != 1:
                    raise NotImplementedError("closing expects 1 argument")
                payload = self.visit(node.args[0])
                return self._emit_closing(payload)
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "os"
                and node.func.attr == "getenv"
            ):
                if node.keywords:
                    raise NotImplementedError("os.getenv does not support keywords")
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("os.getenv expects 1 or 2 arguments")
                key = self.visit(node.args[0])
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="ENV_GET", args=[key, default], result=res))
                return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_json"
            ):
                if node.func.attr == "parse":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if self.parse_codec == "cbor":
                        kind = "CBOR_PARSE"
                    elif self.parse_codec == "json":
                        kind = "JSON_PARSE"
                    else:
                        kind = "MSGPACK_PARSE"
                    self.emit(MoltOp(kind=kind, args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_msgpack"
            ):
                if node.func.attr == "parse":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="MSGPACK_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_cbor"
            ):
                if node.func.attr == "parse":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="CBOR_PARSE", args=[arg], result=res))
                    return res
            if (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "molt_buffer"
            ):
                if node.func.attr == "new":
                    if len(node.args) not in (2, 3):
                        raise NotImplementedError(
                            "molt_buffer.new expects 2 or 3 arguments"
                        )
                    rows = self.visit(node.args[0])
                    cols = self.visit(node.args[1])
                    if len(node.args) == 3:
                        init = self.visit(node.args[2])
                    else:
                        init = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=init))
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_NEW", args=[rows, cols, init], result=res)
                    )
                    return res
                if node.func.attr == "get":
                    if len(node.args) != 3:
                        raise NotImplementedError("molt_buffer.get expects 3 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="BUFFER2D_GET", args=[buf, row, col], result=res)
                    )
                    return res
                if node.func.attr == "set":
                    if len(node.args) != 4:
                        raise NotImplementedError("molt_buffer.set expects 4 arguments")
                    buf = self.visit(node.args[0])
                    row = self.visit(node.args[1])
                    col = self.visit(node.args[2])
                    val = self.visit(node.args[3])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(
                            kind="BUFFER2D_SET", args=[buf, row, col, val], result=res
                        )
                    )
                    return res
                if node.func.attr == "matmul":
                    if len(node.args) != 2:
                        raise NotImplementedError(
                            "molt_buffer.matmul expects 2 arguments"
                        )
                    lhs = self.visit(node.args[0])
                    rhs = self.visit(node.args[1])
                    res = MoltValue(self.next_var(), type_hint="buffer2d")
                    self.emit(
                        MoltOp(kind="BUFFER2D_MATMUL", args=[lhs, rhs], result=res)
                    )
                    return res
            elif (
                isinstance(node.func.value, ast.Name)
                and node.func.value.id == "asyncio"
            ):
                if node.func.attr == "run":
                    arg = self.visit(node.args[0])
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[arg], result=res))
                    return res
                elif node.func.attr == "sleep":
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(kind="CALL_ASYNC", args=["molt_async_sleep"], result=res)
                    )
                    return res

            receiver = self.visit(node.func.value)
            if receiver is None:
                receiver = MoltValue("unknown_obj", type_hint="Unknown")
            method = node.func.attr
            if receiver.type_hint == "generator":
                if method == "send":
                    if len(node.args) != 1:
                        raise NotImplementedError("generator.send expects 1 argument")
                    arg = self.visit(node.args[0])
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="GEN_SEND", args=[receiver, arg], result=pair)
                    )
                    one = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[1], result=one))
                    zero = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                    value = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=value))
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
                    self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
                    self._emit_stop_iteration_from_value(value)
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    return value
                if method == "throw":
                    if len(node.args) != 1:
                        raise NotImplementedError("generator.throw expects 1 argument")
                    arg = self.visit(node.args[0])
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="GEN_THROW", args=[receiver, arg], result=pair)
                    )
                    one = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[1], result=one))
                    zero = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                    value = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=value))
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
                    self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
                    self._emit_stop_iteration_from_value(value)
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    return value
                if method == "close":
                    if node.args:
                        raise NotImplementedError("generator.close expects 0 arguments")
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="GEN_CLOSE", args=[receiver], result=res))
                    return res
            class_name = None
            class_info = self.classes.get(receiver.type_hint)
            if class_info is None and isinstance(node.func.value, ast.Name):
                class_name = node.func.value.id
                class_info = self.classes.get(class_name)
            lookup_class = class_name
            if lookup_class is None and receiver.type_hint in self.classes:
                lookup_class = receiver.type_hint
            method_info = None
            if lookup_class:
                method_info, _ = self._resolve_method_info(lookup_class, method)
            if method_info:
                func_val = method_info["func"]
                descriptor = method_info["descriptor"]
                args = self._emit_call_args(node.args)
                if descriptor == "function":
                    if class_name is None and receiver.type_hint in self.classes:
                        args = [receiver] + args
                elif descriptor == "classmethod":
                    if class_name is None and receiver.type_hint in self.classes:
                        class_name = receiver.type_hint
                    if class_name is None:
                        raise NotImplementedError("Unsupported classmethod call")
                    class_ref = (
                        receiver
                        if isinstance(node.func.value, ast.Name)
                        and class_name == node.func.value.id
                        else self._emit_module_attr_get(class_name)
                    )
                    args = [class_ref] + args
                elif descriptor != "staticmethod":
                    args = []
                if args or descriptor in {"function", "classmethod", "staticmethod"}:
                    res_hint = "Any"
                    return_hint = method_info["return_hint"]
                    if return_hint and return_hint in self.classes:
                        res_hint = return_hint
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    target_name = func_val.type_hint.split(":", 1)[1]
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                    return res
            if method == "append":
                if len(node.args) != 1:
                    raise NotImplementedError("list.append expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_APPEND", args=[receiver, arg], result=res))
                return res
            if method == "extend":
                if len(node.args) != 1:
                    raise NotImplementedError("list.extend expects 1 argument")
                other = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_EXTEND", args=[receiver, other], result=res)
                )
                return res
            if method == "insert":
                if len(node.args) != 2:
                    raise NotImplementedError("list.insert expects 2 arguments")
                idx = self.visit(node.args[0])
                val = self.visit(node.args[1])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_INSERT", args=[receiver, idx, val], result=res)
                )
                return res
            if method == "remove":
                if len(node.args) != 1:
                    raise NotImplementedError("list.remove expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REMOVE", args=[receiver, val], result=res))
                return res
            if method == "count" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.index expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_INDEX", args=[receiver, val], result=res))
                return res
            if method == "pop":
                if receiver.type_hint == "dict":
                    if len(node.args) not in (1, 2):
                        raise NotImplementedError("dict.pop expects 1 or 2 arguments")
                    key = self.visit(node.args[0])
                    if len(node.args) == 2:
                        default = self.visit(node.args[1])
                        has_default = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[1], result=has_default))
                    else:
                        default = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                        has_default = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=has_default))
                    res_type = "Any"
                    if self.type_hint_policy == "trust":
                        hint = self._dict_value_hint(receiver)
                        if hint is not None:
                            res_type = hint
                    res = MoltValue(self.next_var(), type_hint=res_type)
                    self.emit(
                        MoltOp(
                            kind="DICT_POP",
                            args=[receiver, key, default, has_default],
                            result=res,
                        )
                    )
                    return res
                if len(node.args) > 1:
                    raise NotImplementedError("list.pop expects 0 or 1 argument")
                if node.args:
                    idx = self.visit(node.args[0])
                else:
                    idx = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=idx))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="LIST_POP", args=[receiver, idx], result=res))
                return res
            if method == "get":
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("dict.get expects 1 or 2 arguments")
                key = self.visit(node.args[0])
                if len(node.args) == 2:
                    default = self.visit(node.args[1])
                else:
                    default = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                res_type = "Any"
                if self.type_hint_policy == "trust":
                    hint = self._dict_value_hint(receiver)
                    if hint is not None:
                        res_type = hint
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(kind="DICT_GET", args=[receiver, key, default], result=res)
                )
                return res
            if method == "keys":
                res = MoltValue(self.next_var(), type_hint="dict_keys_view")
                self.emit(MoltOp(kind="DICT_KEYS", args=[receiver], result=res))
                return res
            if method == "values":
                res = MoltValue(self.next_var(), type_hint="dict_values_view")
                self.emit(MoltOp(kind="DICT_VALUES", args=[receiver], result=res))
                return res
            if method == "items":
                res = MoltValue(self.next_var(), type_hint="dict_items_view")
                self.emit(MoltOp(kind="DICT_ITEMS", args=[receiver], result=res))
                return res
            if method == "read" and receiver.type_hint.startswith("file"):
                if len(node.args) > 1:
                    raise NotImplementedError("file.read expects 0 or 1 argument")
                if node.args:
                    size_val = self.visit(node.args[0])
                else:
                    size_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=size_val))
                if receiver.type_hint == "file_bytes":
                    res_hint = "bytes"
                elif receiver.type_hint == "file_text":
                    res_hint = "str"
                else:
                    res_hint = "Any"
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="FILE_READ", args=[receiver, size_val], result=res)
                )
                return res
            if method == "write" and receiver.type_hint.startswith("file"):
                if len(node.args) != 1:
                    raise NotImplementedError("file.write expects 1 argument")
                data = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="FILE_WRITE", args=[receiver, data], result=res))
                return res
            if method == "close" and receiver.type_hint.startswith("file"):
                if node.args:
                    raise NotImplementedError("file.close expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FILE_CLOSE", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "tuple":
                if len(node.args) != 1:
                    raise NotImplementedError("tuple.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TUPLE_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "tuple":
                if len(node.args) != 1:
                    raise NotImplementedError("tuple.index expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="TUPLE_INDEX", args=[receiver, val], result=res))
                return res
            if method == "tobytes":
                if node.args:
                    raise NotImplementedError("tobytes expects 0 arguments")
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "memoryview"
                if receiver.type_hint == "memoryview":
                    res = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(
                        MoltOp(kind="MEMORYVIEW_TOBYTES", args=[receiver], result=res)
                    )
                    return res
            if method == "count":
                if len(node.args) != 1:
                    raise NotImplementedError("count expects 1 argument")
                needle = self.visit(node.args[0])
                if (
                    receiver.type_hint in {"Any", "Unknown"}
                    and needle.type_hint == "str"
                ):
                    receiver.type_hint = "str"
                if receiver.type_hint == "str":
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="STRING_COUNT", args=[receiver, needle], result=res)
                    )
                    return res
            if method == "startswith":
                if len(node.args) != 1:
                    raise NotImplementedError("startswith expects 1 argument")
                needle = self.visit(node.args[0])
                if (
                    receiver.type_hint in {"Any", "Unknown"}
                    and needle.type_hint == "str"
                ):
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(
                            kind="STRING_STARTSWITH",
                            args=[receiver, needle],
                            result=res,
                        )
                    )
                    return res
            if method == "endswith":
                if len(node.args) != 1:
                    raise NotImplementedError("endswith expects 1 argument")
                needle = self.visit(node.args[0])
                if (
                    receiver.type_hint in {"Any", "Unknown"}
                    and needle.type_hint == "str"
                ):
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(
                            kind="STRING_ENDSWITH", args=[receiver, needle], result=res
                        )
                    )
                    return res
            if method == "join":
                if len(node.args) != 1:
                    raise NotImplementedError("join expects 1 argument")
                items = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_JOIN", args=[receiver, items], result=res)
                    )
                    return res
            if method == "format":
                if not (
                    isinstance(node.func.value, ast.Constant)
                    and isinstance(node.func.value.value, str)
                ):
                    raise NotImplementedError(
                        "format requires a string literal receiver"
                    )
                fmt_parts = self._parse_format_literal(node.func.value.value)
                fmt_values = [self.visit(arg) for arg in node.args]
                kw_values: dict[str, MoltValue] = {}
                for kw in node.keywords:
                    if kw.arg is None:
                        raise NotImplementedError("format **kwargs are not supported")
                    kw_values[kw.arg] = self.visit(kw.value)
                str_parts: list[MoltValue] = []
                for kind, value, spec in fmt_parts:
                    if kind == "text":
                        if value:
                            lit = MoltValue(self.next_var(), type_hint="str")
                            self.emit(
                                MoltOp(kind="CONST_STR", args=[value], result=lit)
                            )
                            str_parts.append(lit)
                        continue
                    if isinstance(value, int):
                        if value >= len(fmt_values):
                            raise NotImplementedError("format placeholder out of range")
                        item = fmt_values[value]
                    elif isinstance(value, str):
                        if value not in kw_values:
                            raise NotImplementedError(
                                f"format placeholder missing keyword: {value}"
                            )
                        item = kw_values[value]
                    else:
                        raise NotImplementedError(
                            "format placeholder type not supported"
                        )
                    if spec:
                        str_parts.append(self._emit_string_format(item, spec))
                    else:
                        str_parts.append(self._emit_str_from_obj(item))
                return self._emit_string_join(str_parts)
            if method == "split":
                if len(node.args) != 1:
                    raise NotImplementedError("split expects 1 argument")
                needle = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                res = MoltValue(self.next_var(), type_hint="list")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_SPLIT", args=[receiver, needle], result=res)
                    )
                    return res
                if receiver.type_hint == "bytes":
                    self.emit(
                        MoltOp(kind="BYTES_SPLIT", args=[receiver, needle], result=res)
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_SPLIT", args=[receiver, needle], result=res
                        )
                    )
                    return res
            if method == "replace":
                if len(node.args) != 2:
                    raise NotImplementedError("replace expects 2 arguments")
                old = self.visit(node.args[0])
                new = self.visit(node.args[1])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if "str" in {old.type_hint, new.type_hint}:
                        receiver.type_hint = "str"
                    elif "bytearray" in {old.type_hint, new.type_hint}:
                        receiver.type_hint = "bytearray"
                    elif "bytes" in {old.type_hint, new.type_hint}:
                        receiver.type_hint = "bytes"
                res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(
                            kind="STRING_REPLACE",
                            args=[receiver, old, new],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    self.emit(
                        MoltOp(
                            kind="BYTES_REPLACE",
                            args=[receiver, old, new],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_REPLACE",
                            args=[receiver, old, new],
                            result=res,
                        )
                    )
                    return res
            if method == "find":
                if len(node.args) != 1:
                    raise NotImplementedError("find expects 1 argument")
                needle = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                res = MoltValue(self.next_var(), type_hint="int")
                if receiver.type_hint == "bytes":
                    self.emit(
                        MoltOp(kind="BYTES_FIND", args=[receiver, needle], result=res)
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FIND", args=[receiver, needle], result=res
                        )
                    )
                    return res
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_FIND", args=[receiver, needle], result=res)
                    )
                    return res

        if isinstance(node.func, ast.Name):
            func_id = node.func.id
            target_info = self.locals.get(func_id) or self.globals.get(func_id)
            if func_id in {
                "BaseException",
                "Exception",
                "KeyError",
                "IndexError",
                "ValueError",
                "TypeError",
                "RuntimeError",
                "StopIteration",
            }:
                if node.keywords or len(node.args) > 1:
                    self._bridge_fallback(
                        node,
                        f"{func_id} with multiple args/keywords",
                        impact="medium",
                        alternative=f"{func_id} with a single string message",
                        detail="only one positional message is supported",
                    )
                    return None
                msg = ""
                if node.args:
                    arg = node.args[0]
                    if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
                        msg = arg.value
                    else:
                        self._bridge_fallback(
                            node,
                            f"{func_id} with non-string message",
                            impact="medium",
                            alternative=f"{func_id} with a string literal message",
                            detail="non-string messages are not supported yet",
                        )
                        return None
                return self._emit_exception_new(func_id, msg)
            if func_id == "getattr":
                if len(node.args) not in {2, 3} or node.keywords:
                    raise NotImplementedError("getattr expects 2 or 3 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("getattr expects object and name")
                res_hint = "Any"
                name_lit = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    name_lit = node.args[1].value
                if name_lit and obj.type_hint in self.classes:
                    class_info = self.classes[obj.type_hint]
                    if not class_info.get("dynamic"):
                        field_map = class_info.get("fields", {})
                        if name_lit in field_map:
                            if class_info.get("dataclass"):
                                idx_val = MoltValue(self.next_var(), type_hint="int")
                                self.emit(
                                    MoltOp(
                                        kind="CONST",
                                        args=[field_map[name_lit]],
                                        result=idx_val,
                                    )
                                )
                                res = MoltValue(self.next_var())
                                self.emit(
                                    MoltOp(
                                        kind="DATACLASS_GET",
                                        args=[obj, idx_val],
                                        result=res,
                                    )
                                )
                            else:
                                res = MoltValue(self.next_var())
                                self.emit(
                                    MoltOp(
                                        kind="GUARDED_GETATTR",
                                        args=[obj, name_lit, obj.type_hint],
                                        result=res,
                                    )
                                )
                            return res
                if name_lit:
                    class_name = None
                    if obj.type_hint in self.classes:
                        class_name = obj.type_hint
                    elif isinstance(node.args[0], ast.Name):
                        if node.args[0].id in self.classes:
                            class_name = node.args[0].id
                    if class_name:
                        method_info, method_class = self._resolve_method_info(
                            class_name, name_lit
                        )
                        if method_info:
                            descriptor = method_info["descriptor"]
                            if descriptor in {"function", "classmethod"}:
                                method_owner = method_class or class_name
                                res_hint = f"BoundMethod:{method_owner}:{name_lit}"
                            elif descriptor == "staticmethod":
                                res_hint = method_info["func"].type_hint
                res = MoltValue(self.next_var(), type_hint=res_hint)
                if len(node.args) == 3:
                    default = self.visit(node.args[2])
                    if default is None:
                        default = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=default))
                    self.emit(
                        MoltOp(
                            kind="GETATTR_NAME_DEFAULT",
                            args=[obj, name, default],
                            result=res,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="GETATTR_NAME",
                            args=[obj, name],
                            result=res,
                        )
                    )
                return res
            if func_id == "setattr":
                if len(node.args) != 3 or node.keywords:
                    raise NotImplementedError("setattr expects 3 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                val = self.visit(node.args[2])
                if obj is None or name is None or val is None:
                    raise NotImplementedError("setattr expects object, name, value")
                if (
                    isinstance(node.args[1], ast.Constant)
                    and isinstance(node.args[1].value, str)
                    and obj.type_hint in self.classes
                ):
                    attr_name = node.args[1].value
                    class_info = self.classes[obj.type_hint]
                    if not class_info.get("dynamic"):
                        field_map = class_info.get("fields", {})
                        if attr_name in field_map:
                            if class_info.get("dataclass"):
                                idx_val = MoltValue(self.next_var(), type_hint="int")
                                self.emit(
                                    MoltOp(
                                        kind="CONST",
                                        args=[field_map[attr_name]],
                                        result=idx_val,
                                    )
                                )
                                self.emit(
                                    MoltOp(
                                        kind="DATACLASS_SET",
                                        args=[obj, idx_val, val],
                                        result=MoltValue("none"),
                                    )
                                )
                                res = MoltValue(self.next_var(), type_hint="None")
                                self.emit(
                                    MoltOp(kind="CONST_NONE", args=[], result=res)
                                )
                            else:
                                res = MoltValue(self.next_var(), type_hint="None")
                                if self._class_attr_is_data_descriptor(
                                    obj.type_hint, attr_name
                                ):
                                    self.emit(
                                        MoltOp(
                                            kind="SETATTR_GENERIC_PTR",
                                            args=[obj, attr_name, val],
                                            result=res,
                                        )
                                    )
                                else:
                                    self._emit_guarded_setattr(
                                        obj, attr_name, val, obj.type_hint
                                    )
                                    self.emit(
                                        MoltOp(
                                            kind="CONST_NONE",
                                            args=[],
                                            result=res,
                                        )
                                    )
                            return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="SETATTR_NAME",
                        args=[obj, name, val],
                        result=res,
                    )
                )
                return res
            if func_id == "delattr":
                if len(node.args) != 2 or node.keywords:
                    raise NotImplementedError("delattr expects 2 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("delattr expects object and name")
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    res = MoltValue(self.next_var(), type_hint="None")
                    attr_name = node.args[1].value
                    if obj.type_hint in self.classes:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_PTR",
                                args=[obj, attr_name],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="DELATTR_GENERIC_OBJ",
                                args=[obj, attr_name],
                                result=res,
                            )
                        )
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="DELATTR_NAME",
                        args=[obj, name],
                        result=res,
                    )
                )
                return res
            if func_id == "hasattr":
                if len(node.args) != 2 or node.keywords:
                    raise NotImplementedError("hasattr expects 2 arguments")
                obj = self.visit(node.args[0])
                name = self.visit(node.args[1])
                if obj is None or name is None:
                    raise NotImplementedError("hasattr expects object and name")
                if (
                    isinstance(node.args[1], ast.Constant)
                    and isinstance(node.args[1].value, str)
                    and obj.type_hint in self.classes
                ):
                    attr_name = node.args[1].value
                    class_info = self.classes[obj.type_hint]
                    if not class_info.get("dynamic"):
                        field_map = class_info.get("fields", {})
                        if attr_name in field_map:
                            res = MoltValue(self.next_var(), type_hint="bool")
                            self.emit(
                                MoltOp(kind="CONST_BOOL", args=[True], result=res)
                            )
                            return res
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(
                        kind="HASATTR_NAME",
                        args=[obj, name],
                        result=res,
                    )
                )
                return res
            if func_id == "super":
                if node.keywords:
                    raise NotImplementedError("super does not support keywords")
                if len(node.args) == 0:
                    if (
                        self.current_class is None
                        or self.current_method_first_param is None
                    ):
                        raise NotImplementedError(
                            "super() without args is only supported inside class methods"
                        )
                    class_ref = self._emit_module_attr_get(self.current_class)
                    obj = self._load_local_value(self.current_method_first_param)
                    if obj is None:
                        raise NotImplementedError("super() missing method receiver")
                    super_hint = "super"
                    if self.current_class is not None:
                        super_hint = f"super:{self.current_class}"
                    res = MoltValue(self.next_var(), type_hint=super_hint)
                    self.emit(
                        MoltOp(kind="SUPER_NEW", args=[class_ref, obj], result=res)
                    )
                    return res
                if len(node.args) == 2:
                    type_val = self.visit(node.args[0])
                    obj_val = self.visit(node.args[1])
                    if type_val is None or obj_val is None:
                        raise NotImplementedError("super expects type and object")
                    super_hint = "super"
                    if isinstance(node.args[0], ast.Name):
                        super_hint = f"super:{node.args[0].id}"
                    res = MoltValue(self.next_var(), type_hint=super_hint)
                    self.emit(
                        MoltOp(kind="SUPER_NEW", args=[type_val, obj_val], result=res)
                    )
                    return res
                raise NotImplementedError("super expects 0 or 2 arguments")
            if func_id == "classmethod":
                if len(node.args) != 1 or node.keywords:
                    raise NotImplementedError("classmethod expects 1 argument")
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise NotImplementedError("classmethod expects a function")
                res = MoltValue(self.next_var(), type_hint="classmethod")
                self.emit(MoltOp(kind="CLASSMETHOD_NEW", args=[func_val], result=res))
                return res
            if func_id == "staticmethod":
                if len(node.args) != 1 or node.keywords:
                    raise NotImplementedError("staticmethod expects 1 argument")
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise NotImplementedError("staticmethod expects a function")
                res = MoltValue(self.next_var(), type_hint="staticmethod")
                self.emit(MoltOp(kind="STATICMETHOD_NEW", args=[func_val], result=res))
                return res
            if func_id == "property":
                if node.keywords or len(node.args) not in {1, 2, 3}:
                    raise NotImplementedError("property expects 1 to 3 arguments")
                getter = self.visit(node.args[0])
                if getter is None:
                    raise NotImplementedError("property expects a getter")
                setter: MoltValue
                deleter: MoltValue
                if len(node.args) > 1:
                    setter = self.visit(node.args[1])
                    if setter is None:
                        raise NotImplementedError("property setter unsupported")
                else:
                    setter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=setter))
                if len(node.args) > 2:
                    deleter = self.visit(node.args[2])
                    if deleter is None:
                        raise NotImplementedError("property deleter unsupported")
                else:
                    deleter = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=deleter))
                res = MoltValue(self.next_var(), type_hint="property")
                self.emit(
                    MoltOp(
                        kind="PROPERTY_NEW",
                        args=[getter, setter, deleter],
                        result=res,
                    )
                )
                return res
            if func_id == "open":
                if node.keywords:
                    mode_kw = next(
                        (kw.value for kw in node.keywords if kw.arg == "mode"), None
                    )
                    if mode_kw is None or len(node.keywords) > 1:
                        raise NotImplementedError("open only supports mode keyword")
                else:
                    mode_kw = None
                if len(node.args) not in {1, 2}:
                    raise NotImplementedError("open expects 1 or 2 arguments")
                path = self.visit(node.args[0])
                mode_expr = mode_kw if mode_kw is not None else None
                if len(node.args) == 2:
                    if mode_expr is not None:
                        raise NotImplementedError("open received mode twice")
                    mode_expr = node.args[1]
                mode_val: MoltValue
                if mode_expr is None:
                    mode_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=["r"], result=mode_val))
                else:
                    if isinstance(mode_expr, ast.Constant) and isinstance(
                        mode_expr.value, str
                    ):
                        mode_val = MoltValue(self.next_var(), type_hint="str")
                        self.emit(
                            MoltOp(
                                kind="CONST_STR",
                                args=[mode_expr.value],
                                result=mode_val,
                            )
                        )
                    else:
                        mode_val = self.visit(mode_expr)
                mode_hint: str | None = None
                if mode_expr is None:
                    mode_hint = "file_text"
                elif isinstance(mode_expr, ast.Constant) and isinstance(
                    mode_expr.value, str
                ):
                    mode_hint = "file_bytes" if "b" in mode_expr.value else "file_text"
                res = MoltValue(self.next_var(), type_hint=mode_hint or "file")
                self.emit(MoltOp(kind="FILE_OPEN", args=[path, mode_val], result=res))
                return res
            if func_id == "nullcontext":
                if len(node.args) > 1:
                    raise NotImplementedError("nullcontext expects 0 or 1 argument")
                if node.args:
                    payload = self.visit(node.args[0])
                else:
                    payload = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=payload))
                return self._emit_nullcontext(payload)
            if func_id == "closing":
                if len(node.args) != 1:
                    raise NotImplementedError("closing expects 1 argument")
                payload = self.visit(node.args[0])
                return self._emit_closing(payload)
            if func_id == "print":
                if node.keywords:
                    raise NotImplementedError(
                        "print keyword arguments are not supported"
                    )
                if len(node.args) == 0:
                    self.emit(
                        MoltOp(kind="PRINT_NEWLINE", args=[], result=MoltValue("none"))
                    )
                    return None
                args: list[MoltValue] = []
                for expr in node.args:
                    arg = self.visit(expr)
                    if arg is None:
                        arg = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                    args.append(arg)
                if len(args) == 1:
                    self.emit(
                        MoltOp(kind="PRINT", args=[args[0]], result=MoltValue("none"))
                    )
                    return None
                parts = [self._emit_str_from_obj(arg) for arg in args]
                sep = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[" "], result=sep))
                items = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=parts, result=items))
                joined = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="STRING_JOIN", args=[sep, items], result=joined))
                self.emit(MoltOp(kind="PRINT", args=[joined], result=MoltValue("none")))
                return None
            elif func_id == "molt_spawn":
                arg = self.visit(node.args[0])
                self.emit(MoltOp(kind="SPAWN", args=[arg], result=MoltValue("none")))
                return None
            elif func_id == "molt_block_on":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_block_on expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[arg], result=res))
                return res
            elif func_id == "molt_async_sleep":
                if node.keywords or len(node.args) > 1:
                    raise NotImplementedError(
                        "molt_async_sleep expects 0 or 1 arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(kind="CALL_ASYNC", args=["molt_async_sleep"], result=res)
                )
                return res
            elif func_id == "molt_chan_new":
                if node.keywords:
                    raise NotImplementedError("molt_chan_new does not support keywords")
                if len(node.args) > 1:
                    raise NotImplementedError("molt_chan_new expects 0 or 1 argument")
                if node.args:
                    capacity = self.visit(node.args[0])
                    if capacity is None:
                        raise NotImplementedError("Unsupported channel capacity")
                else:
                    capacity = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=capacity))
                res = MoltValue(self.next_var(), type_hint="Channel")
                self.emit(MoltOp(kind="CHAN_NEW", args=[capacity], result=res))
                return res
            elif func_id == "molt_chan_send":
                chan = self.visit(node.args[0])
                val = self.visit(node.args[1])
                chan_slot = None
                val_slot = None
                chan_for_send = chan
                val_for_send = val
                if self.is_async():
                    chan_slot = self._async_local_offset(
                        f"__chan_send_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, chan],
                            result=MoltValue("none"),
                        )
                    )
                    val_slot = self._async_local_offset(
                        f"__chan_send_val_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", val_slot, val],
                            result=MoltValue("none"),
                        )
                    )
                self.state_count += 1
                pending_state_id = self.state_count
                self.emit(
                    MoltOp(
                        kind="STATE_LABEL",
                        args=[pending_state_id],
                        result=MoltValue("none"),
                    )
                )
                pending_state_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST", args=[pending_state_id], result=pending_state_val
                    )
                )
                if self.is_async() and chan_slot is not None and val_slot is not None:
                    chan_for_send = MoltValue(self.next_var(), type_hint="Channel")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", chan_slot],
                            result=chan_for_send,
                        )
                    )
                    val_for_send = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", val_slot],
                            result=val_for_send,
                        )
                    )
                self.state_count += 1
                next_state_id = self.state_count
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_SEND_YIELD",
                        args=[
                            chan_for_send,
                            val_for_send,
                            pending_state_val,
                            next_state_id,
                        ],
                        result=res,
                    )
                )
                if self.is_async() and chan_slot is not None and val_slot is not None:
                    cleared_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", val_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            elif func_id == "molt_chan_recv":
                chan = self.visit(node.args[0])
                chan_slot = None
                chan_for_recv = chan
                if self.is_async():
                    chan_slot = self._async_local_offset(
                        f"__chan_recv_{len(self.async_locals)}"
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, chan],
                            result=MoltValue("none"),
                        )
                    )
                self.state_count += 1
                pending_state_id = self.state_count
                self.emit(
                    MoltOp(
                        kind="STATE_LABEL",
                        args=[pending_state_id],
                        result=MoltValue("none"),
                    )
                )
                pending_state_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST", args=[pending_state_id], result=pending_state_val
                    )
                )
                if self.is_async() and chan_slot is not None:
                    chan_for_recv = MoltValue(self.next_var(), type_hint="Channel")
                    self.emit(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", chan_slot],
                            result=chan_for_recv,
                        )
                    )
                self.state_count += 1
                next_state_id = self.state_count
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CHAN_RECV_YIELD",
                        args=[chan_for_recv, pending_state_val, next_state_id],
                        result=res,
                    )
                )
                if self.is_async() and chan_slot is not None:
                    cleared_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", chan_slot, cleared_val],
                            result=MoltValue("none"),
                        )
                    )
                return res
            class_id = None
            if func_id in self.classes:
                class_id = func_id
            elif target_info and target_info.type_hint in self.classes:
                class_id = target_info.type_hint
            if class_id is not None:
                class_info = self.classes[class_id]
                if class_info.get("dataclass"):
                    if any(kw.arg is None for kw in node.keywords):
                        raise NotImplementedError(
                            "Dataclass **kwargs are not supported"
                        )
                    field_order = class_info["field_order"]
                    defaults = class_info["defaults"]
                    if len(node.args) > len(field_order):
                        raise NotImplementedError(
                            "Too many dataclass positional arguments"
                        )
                    field_values: list[MoltValue] = []
                    kw_values = {
                        kw.arg: self.visit(kw.value)
                        for kw in node.keywords
                        if kw.arg is not None
                    }
                    for idx, name in enumerate(field_order):
                        if idx < len(node.args):
                            val = self.visit(node.args[idx])
                            field_values.append(val)
                            continue
                        if name in kw_values:
                            field_values.append(kw_values[name])
                            continue
                        if name in defaults:
                            field_values.append(self.visit(defaults[name]))
                            continue
                        raise NotImplementedError(f"Missing dataclass field: {name}")
                    extra = set(kw_values) - set(field_order)
                    if extra:
                        raise NotImplementedError(
                            f"Unknown dataclass field(s): {', '.join(sorted(extra))}"
                        )
                    name_val = MoltValue(self.next_var(), type_hint="str")
                    self.emit(
                        MoltOp(kind="CONST_STR", args=[class_id], result=name_val)
                    )
                    field_name_vals: list[MoltValue] = []
                    for field in field_order:
                        field_val = MoltValue(self.next_var(), type_hint="str")
                        self.emit(
                            MoltOp(kind="CONST_STR", args=[field], result=field_val)
                        )
                        field_name_vals.append(field_val)
                    field_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_name_vals,
                            result=field_names_tuple,
                        )
                    )
                    values_tuple = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="TUPLE_NEW",
                            args=field_values,
                            result=values_tuple,
                        )
                    )
                    flags = 0
                    if class_info.get("frozen"):
                        flags |= 0x1
                    if class_info.get("eq"):
                        flags |= 0x2
                    if class_info.get("repr"):
                        flags |= 0x4
                    if class_info.get("slots"):
                        flags |= 0x8
                    flags_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[flags], result=flags_val))
                    res = MoltValue(self.next_var(), type_hint=class_id)
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_NEW",
                            args=[name_val, field_names_tuple, values_tuple, flags_val],
                            result=res,
                        )
                    )
                    class_ref = self._emit_module_attr_get(class_id)
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_SET_CLASS",
                            args=[res, class_ref],
                            result=MoltValue("none"),
                        )
                    )
                    return res
                if node.keywords:
                    raise NotImplementedError("Class **kwargs are not supported")
                res = MoltValue(self.next_var(), type_hint=class_id)
                self.emit(MoltOp(kind="ALLOC", args=[class_id], result=res))
                class_ref = self._emit_module_attr_get(class_id)
                self.emit(
                    MoltOp(
                        kind="OBJECT_SET_CLASS",
                        args=[res, class_ref],
                        result=MoltValue("none"),
                    )
                )
                field_order = class_info.get("field_order") or list(
                    class_info.get("fields", {}).keys()
                )
                defaults = class_info.get("defaults", {})
                for name in field_order:
                    default_expr = defaults.get(name)
                    if default_expr is not None:
                        val = self.visit(default_expr)
                        if val is None:
                            val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                    else:
                        val = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                    if class_info.get("dynamic"):
                        self.emit(
                            MoltOp(
                                kind="SETATTR_GENERIC_PTR",
                                args=[res, name, val],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="SETATTR",
                                args=[res, name, val, class_id],
                                result=MoltValue("none"),
                            )
                        )
                init_method = class_info.get("methods", {}).get("__init__")
                if init_method is None:
                    for base_name in class_info.get("mro", [])[1:]:
                        base_info = self.classes.get(base_name)
                        if base_info and base_info.get("methods", {}).get("__init__"):
                            init_method = base_info["methods"]["__init__"]
                            break
                if init_method is not None:
                    init_func = init_method["func"]
                    target_name = init_func.type_hint.split(":", 1)[1]
                    args = [res] + self._emit_call_args(node.args)
                    init_res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=init_res)
                    )
                    return res
                if node.args:
                    raise NotImplementedError("Class constructor takes no arguments")
                return res

            if target_info and str(target_info.type_hint).startswith("AsyncFunc:"):
                parts = target_info.type_hint.split(":")
                poll_func = parts[1]
                closure_size = int(parts[2])
                args = self._emit_call_args(node.args)
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="ALLOC_FUTURE",
                        args=[poll_func, closure_size] + args,
                        result=res,
                    )
                )
                return res
            if target_info and str(target_info.type_hint).startswith("GenFunc:"):
                parts = target_info.type_hint.split(":")
                poll_func = parts[1]
                closure_size = int(parts[2])
                args = self._emit_call_args(node.args)
                res = MoltValue(self.next_var(), type_hint="generator")
                self.emit(
                    MoltOp(
                        kind="ALLOC_GENERATOR",
                        args=[poll_func, closure_size] + args,
                        result=res,
                    )
                )
                return res

            if target_info and str(target_info.type_hint).startswith("BoundMethod:"):
                res_hint = "Any"
                parts = target_info.type_hint.split(":", 2)
                if len(parts) == 3:
                    class_name = parts[1]
                    method_name = parts[2]
                    method_info = (
                        self.classes.get(class_name, {})
                        .get("methods", {})
                        .get(method_name)
                    )
                    if method_info:
                        return_hint = method_info["return_hint"]
                    if return_hint and return_hint in self.classes:
                        res_hint = return_hint
                args = self._emit_call_args(node.args)
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="CALL_METHOD", args=[target_info] + args, result=res)
                )
                return res

            if target_info and str(target_info.type_hint).startswith("Func:"):
                target_name = target_info.type_hint.split(":")[1]
                args = self._emit_call_args(node.args)
                res = MoltValue(self.next_var(), type_hint="int")
                if self.is_async():
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                else:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    self.emit(
                        MoltOp(
                            kind="CALL_GUARDED",
                            args=[callee] + args,
                            result=res,
                            metadata={"target": target_name},
                        )
                    )
                return res

            if func_id == "type":
                if len(node.args) != 1:
                    raise NotImplementedError("type expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="type")
                self.emit(MoltOp(kind="TYPE_OF", args=[arg], result=res))
                return res
            if func_id == "isinstance":
                if len(node.args) != 2:
                    raise NotImplementedError("isinstance expects 2 arguments")
                obj = self.visit(node.args[0])
                clsinfo = self.visit(node.args[1])
                if obj is None or clsinfo is None:
                    raise NotImplementedError("Unsupported isinstance arguments")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="ISINSTANCE", args=[obj, clsinfo], result=res))
                return res
            if func_id == "issubclass":
                if len(node.args) != 2:
                    raise NotImplementedError("issubclass expects 2 arguments")
                sub = self.visit(node.args[0])
                clsinfo = self.visit(node.args[1])
                if sub is None or clsinfo is None:
                    raise NotImplementedError("Unsupported issubclass arguments")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="ISSUBCLASS", args=[sub, clsinfo], result=res))
                return res
            if func_id == "object":
                if node.args:
                    raise NotImplementedError("object expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="object")
                self.emit(MoltOp(kind="OBJECT_NEW", args=[], result=res))
                return res
            if func_id == "len":
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[arg], result=res))
                return res
            if func_id == "str":
                if len(node.args) > 1:
                    raise NotImplementedError("str expects 0 or 1 arguments")
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="str")
                    self.emit(MoltOp(kind="CONST_STR", args=[""], result=res))
                    return res
                arg = self.visit(node.args[0])
                if arg is None:
                    arg = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                return self._emit_str_from_obj(arg)
            if func_id == "range":
                range_args = self._parse_range_call(node)
                if range_args is None:
                    raise NotImplementedError("Unsupported range invocation")
                start, stop, step = range_args
                res = MoltValue(self.next_var(), type_hint="range")
                self.emit(
                    MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=res)
                )
                return res
            if func_id == "slice":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("slice expects 1-3 arguments")
                if len(node.args) == 1:
                    start = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                    stop = self.visit(node.args[0])
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                elif len(node.args) == 2:
                    start = self.visit(node.args[0])
                    stop = self.visit(node.args[1])
                    step = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                else:
                    start = self.visit(node.args[0])
                    stop = self.visit(node.args[1])
                    step = self.visit(node.args[2])
                res = MoltValue(self.next_var(), type_hint="slice")
                self.emit(
                    MoltOp(kind="SLICE_NEW", args=[start, stop, step], result=res)
                )
                return res
            if func_id == "aiter":
                if len(node.args) != 1:
                    raise NotImplementedError("aiter expects 1 argument")
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported iterable in aiter()")
                return self._emit_aiter(iterable)
            if func_id == "anext":
                # TODO(type-coverage, owner:frontend, milestone:TC2): support returning awaitables outside await.
                self._bridge_fallback(
                    node,
                    "anext outside await",
                    impact="high",
                    alternative="use `await anext(...)` inside async functions",
                    detail="anext lowering is only supported in await expressions",
                )
                return None
            if func_id == "next":
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("next expects 1 or 2 arguments")
                iter_obj = self.visit(node.args[0])
                if iter_obj is None:
                    raise NotImplementedError("Unsupported iterator in next()")
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                is_none = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS", args=[pair, none_val], result=is_none))
                self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
                err_val = self._emit_exception_new(
                    "TypeError", "object is not an iterator"
                )
                self.emit(
                    MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                zero = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=val))
                done = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
                res_cell = MoltValue(self.next_var(), type_hint="list")
                if len(node.args) == 2:
                    default_val = self.visit(node.args[1])
                else:
                    default_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
                self.emit(MoltOp(kind="LIST_NEW", args=[default_val], result=res_cell))
                self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
                if len(node.args) == 1:
                    self._emit_stop_iteration_from_value(val)
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[res_cell, zero, val],
                        result=MoltValue("none"),
                    )
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="INDEX", args=[res_cell, zero], result=res))
                return res
            if func_id == "iter":
                if len(node.args) != 1:
                    raise NotImplementedError("iter expects 1 argument")
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported iterable in iter()")
                return self._emit_iter_new(iterable)
            if func_id == "list":
                if len(node.args) > 1:
                    raise NotImplementedError("list expects 0 or 1 arguments")
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step = range_args
                    range_obj = MoltValue(self.next_var(), type_hint="range")
                    self.emit(
                        MoltOp(
                            kind="RANGE_NEW",
                            args=[start, stop, step],
                            result=range_obj,
                        )
                    )
                    return self._emit_list_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported list input")
                return self._emit_list_from_iter(iterable)
            if func_id == "tuple":
                if len(node.args) > 1:
                    raise NotImplementedError("tuple expects 0 or 1 arguments")
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step = range_args
                    range_obj = MoltValue(self.next_var(), type_hint="range")
                    self.emit(
                        MoltOp(
                            kind="RANGE_NEW",
                            args=[start, stop, step],
                            result=range_obj,
                        )
                    )
                    return self._emit_tuple_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported tuple input")
                if iterable.type_hint == "tuple":
                    return iterable
                if iterable.type_hint == "list":
                    res = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_FROM_LIST", args=[iterable], result=res)
                    )
                    return res
                return self._emit_tuple_from_iter(iterable)
            if func_id == "bytes":
                if len(node.args) > 1:
                    raise NotImplementedError("bytes expects 0 or 1 arguments")
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=res))
                    return res
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported bytes input")
                res = MoltValue(self.next_var(), type_hint="bytes")
                self.emit(MoltOp(kind="BYTES_FROM_OBJ", args=[arg], result=res))
                return res
            if func_id == "bytearray":
                if len(node.args) > 1:
                    raise NotImplementedError("bytearray expects 0 or 1 arguments")
                if node.args:
                    arg = self.visit(node.args[0])
                else:
                    arg = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=arg))
                res = MoltValue(self.next_var(), type_hint="bytearray")
                self.emit(MoltOp(kind="BYTEARRAY_FROM_OBJ", args=[arg], result=res))
                return res
            if func_id == "memoryview":
                if len(node.args) != 1:
                    raise NotImplementedError("memoryview expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="memoryview")
                self.emit(MoltOp(kind="MEMORYVIEW_NEW", args=[arg], result=res))
                return res

            res = MoltValue(self.next_var(), type_hint="Unknown")
            self.emit(MoltOp(kind="CALL_DUMMY", args=[func_id], result=res))
            return res

        callee = self.visit(node.func)
        if callee is None:
            raise NotImplementedError("Unsupported call target")
        if node.keywords:
            raise NotImplementedError("Call keywords are not supported")
        args = self._emit_call_args(node.args)
        res_hint = "Any"
        if callee.type_hint.startswith("BoundMethod:"):
            parts = callee.type_hint.split(":", 2)
            if len(parts) == 3:
                class_name = parts[1]
                method_name = parts[2]
                method_info = (
                    self.classes.get(class_name, {}).get("methods", {}).get(method_name)
                )
                if method_info:
                    return_hint = method_info["return_hint"]
                    if return_hint and return_hint in self.classes:
                        res_hint = return_hint
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_METHOD", args=[callee] + args, result=res))
        elif callee.type_hint.startswith("Func:"):
            func_symbol = callee.type_hint.split(":", 1)[1]
            func_name = self.func_symbol_names.get(func_symbol)
            if func_name and func_name in self.globals:
                expected = self._emit_module_attr_get(func_name)
                matches = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS", args=[callee, expected], result=matches))
                zero = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                init = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=init))
                res_cell = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_NEW", args=[init], result=res_cell))
                self.emit(MoltOp(kind="IF", args=[matches], result=MoltValue("none")))
                direct_res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="CALL", args=[func_symbol] + args, result=direct_res)
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[res_cell, zero, direct_res],
                        result=MoltValue("none"),
                    )
                )
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                fallback_res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee] + args,
                        result=fallback_res,
                    )
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[res_cell, zero, fallback_res],
                        result=MoltValue("none"),
                    )
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(MoltOp(kind="INDEX", args=[res_cell, zero], result=res))
                return res
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL", args=[func_symbol] + args, result=res))
        else:
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
        return res

    def visit_Subscript(self, node: ast.Subscript) -> Any:
        target = self.visit(node.value)
        if isinstance(node.slice, ast.Slice):
            lower = node.slice.lower
            upper = node.slice.upper
            step_val = node.slice.step
            if lower is None:
                start = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
            else:
                start = self.visit(lower)
            if upper is None:
                end = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
            else:
                end = self.visit(upper)
            res_type = "Any"
            if target is not None and target.type_hint in {
                "bytes",
                "bytearray",
                "list",
                "tuple",
                "str",
                "memoryview",
            }:
                res_type = target.type_hint
            if step_val is None:
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(MoltOp(kind="SLICE", args=[target, start, end], result=res))
                return res
            step = self.visit(step_val)
            slice_obj = MoltValue(self.next_var(), type_hint="slice")
            self.emit(
                MoltOp(kind="SLICE_NEW", args=[start, end, step], result=slice_obj)
            )
            res = MoltValue(self.next_var(), type_hint=res_type)
            self.emit(MoltOp(kind="INDEX", args=[target, slice_obj], result=res))
            return res
        index_val = self.visit(node.slice)
        res_type = "Any"
        if target is not None:
            if target.type_hint == "memoryview":
                res_type = "int"
            elif self.type_hint_policy == "trust":
                if target.type_hint in {"list", "tuple"}:
                    elem_hint = self._container_elem_hint(target)
                    if elem_hint:
                        res_type = elem_hint
                elif target.type_hint == "dict":
                    val_hint = self._dict_value_hint(target)
                    if val_hint:
                        res_type = val_hint
        res = MoltValue(self.next_var(), type_hint=res_type)
        self.emit(MoltOp(kind="INDEX", args=[target, index_val], result=res))
        return res
        return None

    def visit_Attribute(self, node: ast.Attribute) -> Any:
        obj = self.visit(node.value)
        if obj is None:
            obj = MoltValue("unknown_obj", type_hint="Unknown")
        if obj.type_hint.startswith("super"):
            super_class = None
            if obj.type_hint == "super":
                super_class = self.current_class
            else:
                super_class = obj.type_hint.split(":", 1)[1]
            if super_class:
                method_info, method_class = self._resolve_super_method_info(
                    super_class, node.attr
                )
                if method_info and method_info["descriptor"] in {
                    "function",
                    "classmethod",
                }:
                    owner_name = method_class or super_class
                    res = MoltValue(
                        self.next_var(),
                        type_hint=f"BoundMethod:{owner_name}:{node.attr}",
                    )
                    self.emit(
                        MoltOp(
                            kind="GETATTR_GENERIC_OBJ",
                            args=[obj, node.attr],
                            result=res,
                        )
                    )
                    return res
        class_info = self.classes.get(obj.type_hint)
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if node.attr not in field_map:
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_OBJ",
                        args=[obj, node.attr],
                        result=res,
                    )
                )
                return res
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_map[node.attr]], result=idx_val))
            res = MoltValue(self.next_var())
            self.emit(MoltOp(kind="DATACLASS_GET", args=[obj, idx_val], result=res))
            return res
        method_info = None
        method_class = None
        if class_info:
            method_info, method_class = self._resolve_method_info(
                obj.type_hint, node.attr
            )
        if method_info and method_info["descriptor"] == "function":
            func_val = method_info["func"]
            class_name = method_class or obj.type_hint
            res = MoltValue(
                self.next_var(),
                type_hint=f"BoundMethod:{class_name}:{node.attr}",
            )
            self.emit(MoltOp(kind="BOUND_METHOD_NEW", args=[func_val, obj], result=res))
            return res
        if obj.type_hint.startswith("module"):
            attr_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[node.attr], result=attr_name))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="MODULE_GET_ATTR",
                    args=[obj, attr_name],
                    result=res,
                )
            )
            return res
        expected_class = obj.type_hint if obj.type_hint in self.classes else None
        res = MoltValue(self.next_var())
        if expected_class is None:
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        if self.classes[expected_class].get("dynamic"):
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        field_map = self.classes[expected_class].get("fields", {})
        if node.attr not in field_map:
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        if self._class_attr_is_data_descriptor(expected_class, node.attr):
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        self.emit(
            MoltOp(
                kind="GUARDED_GETATTR",
                args=[obj, node.attr, expected_class],
                result=res,
            )
        )
        return res

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if not isinstance(node.target, (ast.Name, ast.Attribute)):
            raise NotImplementedError("Only simple annotated assignments are supported")
        hint = None
        if self.type_hint_policy in {"trust", "check"}:
            hint = self._annotation_to_hint(node.annotation)
            if (
                isinstance(node.target, ast.Name)
                and hint is not None
                and node.target.id not in self.explicit_type_hints
            ):
                self.explicit_type_hints[node.target.id] = hint
        if node.value is None:
            return None
        value_node = self.visit(node.value)
        if isinstance(node.target, ast.Name):
            self._apply_explicit_hint(node.target.id, value_node)
            if (
                self.current_func_name == "molt_main"
                or node.target.id not in self.global_decls
            ):
                self._update_exact_local(node.target.id, node.value)
            if (
                self.current_func_name != "molt_main"
                and node.target.id in self.global_decls
            ):
                self._store_local_value(node.target.id, value_node)
                return None
            if self.is_async():
                self._store_local_value(node.target.id, value_node)
            else:
                self._store_local_value(node.target.id, value_node)
                self._emit_module_attr_set(node.target.id, value_node)
                if self.current_func_name == "molt_main":
                    self.globals[node.target.id] = value_node
            return None

        obj = self.visit(node.target.value)
        exact_class = None
        if isinstance(node.target.value, ast.Name):
            exact_class = self.exact_locals.get(node.target.value.id)
        class_info = None
        if obj is not None:
            class_info = self.classes.get(obj.type_hint)
        if exact_class is not None and obj is not None:
            exact_info = self.classes.get(exact_class)
            if (
                exact_info
                and not exact_info.get("dynamic")
                and not exact_info.get("dataclass")
            ):
                field_map = exact_info.get("fields", {})
                if (
                    node.target.attr in field_map
                    and not self._class_attr_is_data_descriptor(
                        exact_class, node.target.attr
                    )
                ):
                    self.emit(
                        MoltOp(
                            kind="SETATTR",
                            args=[obj, node.target.attr, value_node, exact_class],
                            result=MoltValue("none"),
                        )
                    )
                    return None
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if node.target.attr not in field_map:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, node.target.attr, value_node],
                        result=MoltValue("none"),
                    )
                )
                return None
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[field_map[node.target.attr]], result=idx_val)
            )
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
                    result=MoltValue("none"),
                )
            )
        else:
            field_map = class_info.get("fields", {}) if class_info else {}
            if obj is not None and obj.type_hint in self.classes:
                if class_info and class_info.get("dynamic"):
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, node.target.attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
                elif node.target.attr in field_map:
                    if self._class_attr_is_data_descriptor(
                        obj.type_hint, node.target.attr
                    ):
                        self.emit(
                            MoltOp(
                                kind="SETATTR_GENERIC_PTR",
                                args=[obj, node.target.attr, value_node],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        self._emit_guarded_setattr(
                            obj, node.target.attr, value_node, obj.type_hint
                        )
                else:
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, node.target.attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
            else:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, node.target.attr, value_node],
                        result=MoltValue("none"),
                    )
                )
        return None

    def visit_Assign(self, node: ast.Assign) -> None:
        value_node = self.visit(node.value)
        for target in node.targets:
            if isinstance(target, ast.Attribute):
                obj = self.visit(target.value)
                exact_class = None
                if isinstance(target.value, ast.Name):
                    exact_class = self.exact_locals.get(target.value.id)
                class_info = None
                if obj is not None:
                    class_info = self.classes.get(obj.type_hint)
                if exact_class is not None and obj is not None:
                    exact_info = self.classes.get(exact_class)
                    if (
                        exact_info
                        and not exact_info.get("dynamic")
                        and not exact_info.get("dataclass")
                    ):
                        field_map = exact_info.get("fields", {})
                        if (
                            target.attr in field_map
                            and not self._class_attr_is_data_descriptor(
                                exact_class, target.attr
                            )
                        ):
                            self.emit(
                                MoltOp(
                                    kind="SETATTR",
                                    args=[obj, target.attr, value_node, exact_class],
                                    result=MoltValue("none"),
                                )
                            )
                            continue
                if class_info and class_info.get("dataclass"):
                    field_map = class_info["fields"]
                    if target.attr not in field_map:
                        self.emit(
                            MoltOp(
                                kind="SETATTR_GENERIC_OBJ",
                                args=[obj, target.attr, value_node],
                                result=MoltValue("none"),
                            )
                        )
                        continue
                    idx_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="CONST", args=[field_map[target.attr]], result=idx_val
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_SET",
                            args=[obj, idx_val, value_node],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    field_map = class_info.get("fields", {}) if class_info else {}
                    if obj is not None and obj.type_hint in self.classes:
                        if class_info and class_info.get("dynamic"):
                            self.emit(
                                MoltOp(
                                    kind="SETATTR_GENERIC_PTR",
                                    args=[obj, target.attr, value_node],
                                    result=MoltValue("none"),
                                )
                            )
                        elif target.attr in field_map:
                            if self._class_attr_is_data_descriptor(
                                obj.type_hint, target.attr
                            ):
                                self.emit(
                                    MoltOp(
                                        kind="SETATTR_GENERIC_PTR",
                                        args=[obj, target.attr, value_node],
                                        result=MoltValue("none"),
                                    )
                                )
                            else:
                                self._emit_guarded_setattr(
                                    obj, target.attr, value_node, obj.type_hint
                                )
                        else:
                            self.emit(
                                MoltOp(
                                    kind="SETATTR_GENERIC_PTR",
                                    args=[obj, target.attr, value_node],
                                    result=MoltValue("none"),
                                )
                            )
                    else:
                        self.emit(
                            MoltOp(
                                kind="SETATTR_GENERIC_OBJ",
                                args=[obj, target.attr, value_node],
                                result=MoltValue("none"),
                            )
                        )
            elif isinstance(target, ast.Name):
                if (
                    self.current_func_name == "molt_main"
                    or target.id not in self.global_decls
                ):
                    self._update_exact_local(target.id, node.value)
                if (
                    self.current_func_name != "molt_main"
                    and target.id in self.global_decls
                ):
                    self._store_local_value(target.id, value_node)
                    continue
                if self.is_async():
                    self._store_local_value(target.id, value_node)
                else:
                    self._apply_explicit_hint(target.id, value_node)
                    self._store_local_value(target.id, value_node)
                    if value_node is not None:
                        self._propagate_container_hints(target.id, value_node)
                    self._emit_module_attr_set(target.id, value_node)
                    if self.current_func_name == "molt_main":
                        self.globals[target.id] = value_node
            elif isinstance(target, ast.Subscript):
                target_obj = self.visit(target.value)
                if isinstance(target.slice, ast.Slice):
                    if target_obj is None or target_obj.type_hint != "memoryview":
                        raise NotImplementedError("Slice assignment is not supported")
                    if target.slice.lower is None:
                        start = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
                    else:
                        start = self.visit(target.slice.lower)
                    if target.slice.upper is None:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                    else:
                        end = self.visit(target.slice.upper)
                    if target.slice.step is None:
                        step = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
                    else:
                        step = self.visit(target.slice.step)
                    slice_obj = MoltValue(self.next_var(), type_hint="slice")
                    self.emit(
                        MoltOp(
                            kind="SLICE_NEW",
                            args=[start, end, step],
                            result=slice_obj,
                        )
                    )
                    self.emit(
                        MoltOp(
                            kind="STORE_INDEX",
                            args=[target_obj, slice_obj, value_node],
                            result=MoltValue("none"),
                        )
                    )
                    continue
                index_val = self.visit(target.slice)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[target_obj, index_val, value_node],
                        result=MoltValue("none"),
                    )
                )
        return None

    def visit_Delete(self, node: ast.Delete) -> None:
        for target in node.targets:
            if not isinstance(target, ast.Attribute):
                raise NotImplementedError("del only supports attribute deletion")
            obj = self.visit(target.value)
            if obj is None:
                raise NotImplementedError("del expects attribute owner")
            res = MoltValue(self.next_var(), type_hint="None")
            if obj.type_hint in self.classes:
                self.emit(
                    MoltOp(
                        kind="DELATTR_GENERIC_PTR",
                        args=[obj, target.attr],
                        result=res,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="DELATTR_GENERIC_OBJ",
                        args=[obj, target.attr],
                        result=res,
                    )
                )
        return None

    def visit_AugAssign(self, node: ast.AugAssign) -> None:
        if isinstance(node.target, ast.Name):
            self.exact_locals.pop(node.target.id, None)
            load_node = ast.Name(id=node.target.id, ctx=ast.Load())
            may_yield = self._expr_may_yield(node.value)
            if may_yield and self.is_async() and node.target.id in self.async_locals:
                value_node = self.visit(node.value)
                current = self._load_local_value(node.target.id)
            else:
                current = self.visit(load_node)
                value_node = self.visit(node.value)
            if current is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            if isinstance(node.op, ast.Add):
                op_kind = "ADD"
            elif isinstance(node.op, ast.Sub):
                op_kind = "SUB"
            elif isinstance(node.op, ast.Mult):
                op_kind = "MUL"
            else:
                raise NotImplementedError("Unsupported augmented assignment operator")
            res = MoltValue(self.next_var(), type_hint=current.type_hint)
            self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
            if (
                self.current_func_name != "molt_main"
                and node.target.id in self.global_decls
            ):
                self._store_local_value(node.target.id, res)
                return None
            if self.is_async():
                self._store_local_value(node.target.id, res)
            else:
                self._apply_explicit_hint(node.target.id, res)
                self._store_local_value(node.target.id, res)
                if res is not None:
                    self._propagate_container_hints(node.target.id, res)
                self._emit_module_attr_set(node.target.id, res)
                if self.current_func_name == "molt_main":
                    self.globals[node.target.id] = res
            return None
        if isinstance(node.target, ast.Attribute):
            current = self.visit(node.target)
            if current is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            value_node = self.visit(node.value)
            if isinstance(node.op, ast.Add):
                op_kind = "ADD"
            elif isinstance(node.op, ast.Sub):
                op_kind = "SUB"
            elif isinstance(node.op, ast.Mult):
                op_kind = "MUL"
            else:
                raise NotImplementedError("Unsupported augmented assignment operator")
            res = MoltValue(self.next_var(), type_hint=current.type_hint)
            self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
            obj = self.visit(node.target.value)
            exact_class = None
            if isinstance(node.target.value, ast.Name):
                exact_class = self.exact_locals.get(node.target.value.id)
            class_info = None
            if obj is not None:
                class_info = self.classes.get(obj.type_hint)
            if exact_class is not None and obj is not None:
                exact_info = self.classes.get(exact_class)
                if (
                    exact_info
                    and not exact_info.get("dynamic")
                    and not exact_info.get("dataclass")
                ):
                    field_map = exact_info.get("fields", {})
                    if (
                        node.target.attr in field_map
                        and not self._class_attr_is_data_descriptor(
                            exact_class, node.target.attr
                        )
                    ):
                        self.emit(
                            MoltOp(
                                kind="SETATTR",
                                args=[obj, node.target.attr, res, exact_class],
                                result=MoltValue("none"),
                            )
                        )
                        return None
            if class_info and class_info.get("dataclass"):
                field_map = class_info["fields"]
                if node.target.attr not in field_map:
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_OBJ",
                            args=[obj, node.target.attr, res],
                            result=MoltValue("none"),
                        )
                    )
                    return None
                idx_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="CONST",
                        args=[field_map[node.target.attr]],
                        result=idx_val,
                    )
                )
                self.emit(
                    MoltOp(
                        kind="DATACLASS_SET",
                        args=[obj, idx_val, res],
                        result=MoltValue("none"),
                    )
                )
                return None
            field_map = class_info.get("fields", {}) if class_info else {}
            if obj is not None and obj.type_hint in self.classes:
                if class_info and class_info.get("dynamic"):
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, node.target.attr, res],
                            result=MoltValue("none"),
                        )
                    )
                elif node.target.attr in field_map:
                    if self._class_attr_is_data_descriptor(
                        obj.type_hint, node.target.attr
                    ):
                        self.emit(
                            MoltOp(
                                kind="SETATTR_GENERIC_PTR",
                                args=[obj, node.target.attr, res],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        self._emit_guarded_setattr(
                            obj, node.target.attr, res, obj.type_hint
                        )
                else:
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, node.target.attr, res],
                            result=MoltValue("none"),
                        )
                    )
            else:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, node.target.attr, res],
                        result=MoltValue("none"),
                    )
                )
            return None
        raise NotImplementedError("Unsupported augmented assignment target")

    def visit_Compare(self, node: ast.Compare) -> Any:
        left = self.visit(node.left)
        if left is None:
            raise NotImplementedError("Unsupported compare left operand")
        comp_yields = [self._expr_may_yield(comp) for comp in node.comparators]
        left_slot: int | None = None
        if self.is_async() and comp_yields[0]:
            left_slot = self._spill_async_value(
                left, f"__cmp_left_{len(self.async_locals)}"
            )
        right = self.visit(node.comparators[0])
        if right is None:
            raise NotImplementedError("Unsupported compare right operand")
        if left_slot is not None:
            left = self._reload_async_value(left_slot, left.type_hint)
        if len(node.ops) == 1:
            return self._emit_compare_op(node.ops[0], left, right)
        first_cmp = self._emit_compare_op(node.ops[0], left, right)
        result_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[first_cmp], result=result_cell))
        prev_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[right], result=prev_cell))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
        res_slot: int | None = None
        prev_slot: int | None = None
        idx_slot: int | None = None
        if self.is_async() and any(comp_yields[1:]):
            res_slot = self._spill_async_value(
                result_cell, f"__cmp_res_{len(self.async_locals)}"
            )
            prev_slot = self._spill_async_value(
                prev_cell, f"__cmp_prev_{len(self.async_locals)}"
            )
            idx_slot = self._spill_async_value(
                idx, f"__cmp_idx_{len(self.async_locals)}"
            )
        for op, comparator in zip(node.ops[1:], node.comparators[1:]):
            may_yield = self._expr_may_yield(comparator)
            current = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[result_cell, idx], result=current))
            self.emit(MoltOp(kind="IF", args=[current], result=MoltValue("none")))
            prev_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[prev_cell, idx], result=prev_val))
            right_val = self.visit(comparator)
            if right_val is None:
                raise NotImplementedError("Unsupported compare right operand")
            idx_val = idx
            if (
                self.is_async()
                and may_yield
                and res_slot is not None
                and prev_slot is not None
                and idx_slot is not None
            ):
                result_cell = self._reload_async_value(res_slot, "list")
                prev_cell = self._reload_async_value(prev_slot, "list")
                idx_val = self._reload_async_value(idx_slot, "int")
                prev_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="INDEX", args=[prev_cell, idx_val], result=prev_val)
                )
            cmp_val = self._emit_compare_op(op, prev_val, right_val)
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[result_cell, idx_val, cmp_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[prev_cell, idx_val, right_val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        final = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[result_cell, idx], result=final))
        return final

    def visit_UnaryOp(self, node: ast.UnaryOp) -> Any:
        operand = self.visit(node.operand)
        if isinstance(node.op, ast.UAdd):
            return operand
        if isinstance(node.op, ast.USub):
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="SUB", args=[zero, operand], result=res))
            return res
        if isinstance(node.op, ast.Not):
            return self._emit_not(operand)
        raise NotImplementedError("Unary operator not supported")

    def visit_IfExp(self, node: ast.IfExp) -> Any:
        cond = self.visit(node.test)
        if cond is None:
            raise NotImplementedError("Unsupported if expression condition")
        use_phi = self.enable_phi and not self.is_async()
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
            true_val = self.visit(node.body)
            if true_val is None:
                raise NotImplementedError("Unsupported if expression true branch")
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            false_val = self.visit(node.orelse)
            if false_val is None:
                raise NotImplementedError("Unsupported if expression false branch")
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_type = "Any"
            if true_val.type_hint == false_val.type_hint:
                res_type = true_val.type_hint
            merged = MoltValue(self.next_var(), type_hint=res_type)
            self.emit(MoltOp(kind="PHI", args=[true_val, false_val], result=merged))
            return merged

        placeholder = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[placeholder], result=cell))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
        cell_slot: int | None = None
        idx_slot: int | None = None
        if self.is_async() and (
            self._expr_may_yield(node.body) or self._expr_may_yield(node.orelse)
        ):
            cell_slot = self._spill_async_value(
                cell, f"__ifexp_cell_{len(self.async_locals)}"
            )
            idx_slot = self._spill_async_value(
                idx, f"__ifexp_idx_{len(self.async_locals)}"
            )

        self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
        true_val = self.visit(node.body)
        if true_val is None:
            raise NotImplementedError("Unsupported if expression true branch")
        store_cell = cell
        store_idx = idx
        if cell_slot is not None and idx_slot is not None:
            store_cell = self._reload_async_value(cell_slot, "list")
            store_idx = self._reload_async_value(idx_slot, "int")
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[store_cell, store_idx, true_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        false_val = self.visit(node.orelse)
        if false_val is None:
            raise NotImplementedError("Unsupported if expression false branch")
        store_cell = cell
        store_idx = idx
        if cell_slot is not None and idx_slot is not None:
            store_cell = self._reload_async_value(cell_slot, "list")
            store_idx = self._reload_async_value(idx_slot, "int")
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[store_cell, store_idx, false_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        final_cell = cell
        final_idx = idx
        if cell_slot is not None and idx_slot is not None:
            final_cell = self._reload_async_value(cell_slot, "list")
            final_idx = self._reload_async_value(idx_slot, "int")
        res_type = "Any"
        if true_val.type_hint == false_val.type_hint:
            res_type = true_val.type_hint
        result = MoltValue(self.next_var(), type_hint=res_type)
        self.emit(MoltOp(kind="INDEX", args=[final_cell, final_idx], result=result))
        return result

    def visit_If(self, node: ast.If) -> None:
        cond = self.visit(node.test)
        self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
        for item in node.body:
            self.visit(item)
        if node.orelse:
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            for item in node.orelse:
                self.visit(item)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return None

    def visit_With(self, node: ast.With) -> None:
        if self.is_async():
            self._bridge_fallback(
                node,
                "async with",
                impact="high",
                alternative="avoid async context managers or use explicit try/finally",
                detail="async with lowering is not implemented yet",
            )
            return None
        if len(node.items) != 1:
            self._bridge_fallback(
                node,
                "with (multiple context managers)",
                impact="high",
                alternative="nest with blocks",
                detail="only a single context manager is supported",
            )
            return None

        item = node.items[0]
        ctx_val = self.visit(item.context_expr)
        if ctx_val is None:
            self._bridge_fallback(
                node,
                "with",
                impact="high",
                alternative="use contextlib.nullcontext for now",
                detail="context expression did not lower",
            )
            return None

        enter_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONTEXT_ENTER", args=[ctx_val], result=enter_val))
        if item.optional_vars is not None:
            if not isinstance(item.optional_vars, ast.Name):
                self._bridge_fallback(
                    item.optional_vars,
                    "with (destructuring targets)",
                    impact="high",
                    alternative="bind to a single name",
                    detail="only simple name targets are supported",
                )
                return None
            self._store_local_value(item.optional_vars.id, enter_val)

        self.context_depth += 1
        for stmt in node.body:
            self.visit(stmt)
        self.context_depth -= 1

        exc_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=exc_val))
        self.emit(
            MoltOp(
                kind="CONTEXT_EXIT", args=[ctx_val, exc_val], result=MoltValue("none")
            )
        )
        return None

    def visit_For(self, node: ast.For) -> None:
        if node.orelse:
            raise NotImplementedError("for-else is not supported")
        matmul_match = self._match_matmul_loop(node)
        if matmul_match is not None:
            out_name, a_name, b_name = matmul_match
            a_val = self.locals.get(a_name) or self.globals.get(a_name)
            b_val = self.locals.get(b_name) or self.globals.get(b_name)
            if a_val is None or b_val is None:
                raise NotImplementedError("Matmul operands must be simple locals")
            res = MoltValue(self.next_var(), type_hint="buffer2d")
            self.emit(MoltOp(kind="BUFFER2D_MATMUL", args=[a_val, b_val], result=res))
            self.locals[out_name] = res
            return None
        if not isinstance(node.target, ast.Name):
            raise NotImplementedError("Only simple for targets are supported")
        self.exact_locals.pop(node.target.id, None)
        assigned = self._collect_assigned_names(node.body)
        assigned.add(node.target.id)
        for name in sorted(assigned):
            if not self.is_async():
                self._box_local(name)
        indexed_reduction = (
            None if self.is_async() else self._match_indexed_vector_reduction_loop(node)
        )
        if indexed_reduction is None:
            indexed_reduction = (
                None
                if self.is_async()
                else self._match_indexed_vector_minmax_loop(node)
            )
        if indexed_reduction is not None:
            acc_name, seq_name, kind, start_expr = indexed_reduction
            if seq_name in assigned:
                indexed_reduction = None
            else:
                seq_val = self.locals.get(seq_name) or self.globals.get(seq_name)
                if seq_val and seq_val.type_hint in {"list", "tuple"}:
                    acc_val = self._load_local_value(acc_name)
                    if acc_val is not None:
                        elem_hint = self._container_elem_hint(seq_val)
                        vec_kind = {
                            "sum": "VEC_SUM_INT",
                            "prod": "VEC_PROD_INT",
                            "min": "VEC_MIN_INT",
                            "max": "VEC_MAX_INT",
                        }.get(kind, "VEC_SUM_INT")
                        seq_arg = seq_val
                        if kind == "prod" and elem_hint == "int":
                            seq_arg = self._emit_intarray_from_seq(seq_val)
                        if start_expr is not None:
                            vec_kind = f"{vec_kind}_RANGE"
                        if self.type_hint_policy == "trust" and elem_hint == "int":
                            vec_kind = f"{vec_kind}_TRUSTED"
                        zero = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                        one = MoltValue(self.next_var(), type_hint="int")
                        self.emit(MoltOp(kind="CONST", args=[1], result=one))
                        pair = MoltValue(self.next_var(), type_hint="tuple")
                        args = [seq_arg, acc_val]
                        if start_expr is not None:
                            start_val = self.visit(start_expr)
                            if start_val is None:
                                raise NotImplementedError(
                                    "Unsupported range start for vector reduction"
                                )
                            args.append(start_val)
                        self.emit(MoltOp(kind=vec_kind, args=args, result=pair))
                        sum_val = MoltValue(self.next_var(), type_hint="int")
                        self.emit(
                            MoltOp(kind="INDEX", args=[pair, zero], result=sum_val)
                        )
                        ok_val = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=ok_val))
                        self.emit(
                            MoltOp(kind="IF", args=[ok_val], result=MoltValue("none"))
                        )
                        self._store_local_value(acc_name, sum_val)
                        self.emit(
                            MoltOp(kind="ELSE", args=[], result=MoltValue("none"))
                        )
                        range_args = self._parse_range_call(node.iter)
                        if range_args is None:
                            raise NotImplementedError("Unsupported range invocation")
                        start, stop, step = range_args
                        self._emit_range_loop(node, start, stop, step)
                        self.emit(
                            MoltOp(kind="END_IF", args=[], result=MoltValue("none"))
                        )
                        return None
        range_args = self._parse_range_call(node.iter)
        if range_args is not None:
            start, stop, step = range_args
            self._emit_range_loop(node, start, stop, step)
            return None
        iterable = self.visit(node.iter)
        if iterable is None:
            raise NotImplementedError("Unsupported iterable in for loop")
        vector_info = (
            None if self.is_async() else self._match_vector_reduction_loop(node)
        )
        minmax_info = None if self.is_async() else self._match_vector_minmax_loop(node)
        if vector_info is None:
            vector_info = minmax_info
        if (
            vector_info
            and iterable.type_hint in {"list", "tuple"}
            and self._iterable_is_indexable(iterable)
        ):
            acc_name, _, kind = vector_info
            acc_val = self._load_local_value(acc_name)
            if acc_val is not None:
                elem_hint = self._container_elem_hint(iterable)
                vec_kind = {
                    "sum": "VEC_SUM_INT",
                    "prod": "VEC_PROD_INT",
                    "min": "VEC_MIN_INT",
                    "max": "VEC_MAX_INT",
                }.get(kind, "VEC_SUM_INT")
                seq_arg = iterable
                if kind == "prod" and elem_hint == "int":
                    seq_arg = self._emit_intarray_from_seq(iterable)
                if self.type_hint_policy == "trust" and elem_hint == "int":
                    vec_kind = f"{vec_kind}_TRUSTED"
                zero = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                one = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[1], result=one))
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind=vec_kind, args=[seq_arg, acc_val], result=pair))
                sum_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=sum_val))
                ok_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="INDEX", args=[pair, one], result=ok_val))
                self.emit(MoltOp(kind="IF", args=[ok_val], result=MoltValue("none")))
                self._store_local_value(acc_name, sum_val)
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self._emit_for_loop(node, iterable)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                return None

        self._emit_for_loop(node, iterable)
        return None

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        if not self.is_async():
            raise NotImplementedError("async for is only supported in async functions")
        if node.orelse:
            raise NotImplementedError("async for-else is not supported")
        if not isinstance(node.target, ast.Name):
            raise NotImplementedError("Only simple async for targets are supported")
        self.exact_locals.pop(node.target.id, None)
        iterable = self.visit(node.iter)
        if iterable is None:
            raise NotImplementedError("Unsupported iterable in async for loop")
        if node.target.id not in self.async_locals:
            self._async_local_offset(node.target.id)
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_for_iter_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", iter_slot, iter_obj],
                result=MoltValue("none"),
            )
        )
        sentinel = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=sentinel))
        sentinel_slot = self._async_local_offset(
            f"__async_for_sentinel_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", sentinel_slot, sentinel],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        iter_val = MoltValue(self.next_var(), type_hint=iter_obj.type_hint)
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", iter_slot],
                result=iter_val,
            )
        )
        sentinel_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_val,
            )
        )
        item_val = self._emit_await_anext(
            iter_val, default_val=sentinel_val, has_default=True
        )
        sentinel_after = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", sentinel_slot],
                result=sentinel_after,
            )
        )
        is_done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[item_val, sentinel_after], result=is_done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[is_done], result=MoltValue("none"))
        )
        self._store_local_value(node.target.id, item_val)
        for stmt in node.body:
            self.visit(stmt)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return None

    def visit_While(self, node: ast.While) -> None:
        if node.orelse:
            raise NotImplementedError("while-else is not supported")
        counted = self._match_counted_while(node)
        if counted is not None and not self.is_async():
            index_name, bound, body = counted
            acc_name = self._match_counted_while_sum(index_name, body)
            if acc_name is not None:
                start_val = self._load_local_value(index_name)
                if start_val is None:
                    start_const = 0
                else:
                    start_const = self.const_ints.get(start_val.name)
                acc_val = self._load_local_value(acc_name)
                acc_const = None
                if acc_val is not None:
                    acc_const = self.const_ints.get(acc_val.name)
                if start_const is not None and acc_const is not None:
                    span = bound - start_const
                    sum_val = span * (start_const + bound - 1) // 2
                    final_val = acc_const + sum_val
                    acc_res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[final_val], result=acc_res))
                    self._store_local_value(acc_name, acc_res)
                    final_index = bound if start_const < bound else start_const
                    idx_res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[final_index], result=idx_res))
                    self._store_local_value(index_name, idx_res)
                    return None
            assigned = self._collect_assigned_names(node.body)
            for name in sorted(assigned):
                self._box_local(name)
            self._emit_counted_while(index_name, bound, body)
            return None
        assigned = self._collect_assigned_names(node.body)
        for name in sorted(assigned):
            if not self.is_async():
                self._box_local(name)
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        cond = self.visit(node.test)
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        for item in node.body:
            self.visit(item)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return None

    def _emit_guarded_body(
        self, body: list[ast.stmt], baseline_exc: MoltValue | None
    ) -> None:
        if not body:
            return
        self.visit(body[0])
        remaining = body[1:]
        if not remaining:
            return
        exc_after = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_after, none_val], result=is_none))
        if baseline_exc is None:
            pending = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))
            self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._emit_guarded_body(remaining, baseline_exc)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return
        is_same = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_after, baseline_exc], result=is_same))
        continue_guard = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="OR", args=[is_none, is_same], result=continue_guard))
        self.emit(MoltOp(kind="IF", args=[continue_guard], result=MoltValue("none")))
        self._emit_guarded_body(remaining, baseline_exc)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_finalbody(
        self, finalbody: list[ast.stmt], baseline_exc: MoltValue | None
    ) -> None:
        self.return_unwind_depth += 1
        self._emit_guarded_body(finalbody, baseline_exc)
        self.return_unwind_depth -= 1

    def _ctx_mark_arg(self, scope: TryScope) -> MoltValue:
        if scope.ctx_mark_offset is None or not self.is_async():
            return scope.ctx_mark
        res = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", scope.ctx_mark_offset],
                result=res,
            )
        )
        return res

    def _emit_raise_exit(self) -> None:
        if self.try_end_labels:
            if (
                self.try_suppress_depth is None
                or len(self.try_end_labels) > self.try_suppress_depth
            ):
                self.emit(
                    MoltOp(
                        kind="JUMP",
                        args=[self.try_end_labels[-1]],
                        result=MoltValue("none"),
                    )
                )
            return
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        self.emit(MoltOp(kind="ret", args=[none_val], result=MoltValue("none")))

    def _emit_raise_if_pending(self, *, emit_exit: bool = False) -> None:
        exc_after = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
        none_after = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
        is_none_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after))
        pending_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none_after], result=pending_after))
        self.emit(MoltOp(kind="IF", args=[pending_after], result=MoltValue("none")))
        if self.in_generator:
            kind_after = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="EXCEPTION_KIND", args=[exc_after], result=kind_after)
            )
            gen_exit = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=["GeneratorExit"], result=gen_exit))
            is_gen_exit = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="EQ", args=[kind_after, gen_exit], result=is_gen_exit)
            )
            self.emit(MoltOp(kind="IF", args=[is_gen_exit], result=MoltValue("none")))
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        else:
            self.emit(MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none")))
            if emit_exit:
                self._emit_raise_exit()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def visit_Try(self, node: ast.Try) -> None:
        if not node.handlers and not node.finalbody:
            self._bridge_fallback(
                node,
                "try without except",
                impact="high",
                alternative="add an except handler or a finally block",
                detail="try without except/finally is not supported yet",
            )
            return None
        if node.orelse and not node.handlers:
            self._bridge_fallback(
                node,
                "try/finally with else",
                impact="high",
                alternative="move the else body into the try",
                detail="try/else requires an except handler",
            )
            return None

        ctx_mark = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONTEXT_DEPTH", args=[], result=ctx_mark))
        ctx_mark_offset = None
        if self.is_async():
            ctx_name = f"__ctx_mark_{len(self.async_locals)}"
            ctx_mark_offset = self._async_local_offset(ctx_name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", ctx_mark_offset, ctx_mark],
                    result=MoltValue("none"),
                )
            )
        scope = TryScope(
            ctx_mark=ctx_mark,
            finalbody=node.finalbody,
            ctx_mark_offset=ctx_mark_offset,
        )
        self.try_scopes.append(scope)

        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_end_label = self.next_label()
        self.try_end_labels.append(try_end_label)
        self.emit(MoltOp(kind="TRY_START", args=[], result=MoltValue("none")))
        for stmt in node.body:
            self.visit(stmt)
        self.emit(
            MoltOp(
                kind="LABEL",
                args=[try_end_label],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="TRY_END", args=[], result=MoltValue("none")))
        self.try_end_labels.pop()
        prior_suppress = self.try_suppress_depth
        self.try_suppress_depth = len(self.try_end_labels)

        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
        pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))

        self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
        ctx_arg = self._ctx_mark_arg(scope)
        self.emit(
            MoltOp(
                kind="CONTEXT_UNWIND_TO",
                args=[ctx_arg, exc_val],
                result=MoltValue("none"),
            )
        )

        def emit_handlers(handlers: list[ast.ExceptHandler]) -> None:
            if not handlers:
                return
            handler = handlers[0]
            match_val = self._emit_exception_match(handler, exc_val)
            self.emit(MoltOp(kind="IF", args=[match_val], result=MoltValue("none")))
            exc_slot_offset = None
            if self.is_async():
                exc_slot_name = f"__exc_handler_{len(self.async_locals)}"
                exc_slot_offset = self._async_local_offset(exc_slot_name)
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", exc_slot_offset, exc_val],
                        result=MoltValue("none"),
                    )
                )
            if handler.name:
                self._store_local_value(handler.name, exc_val)
            self.active_exceptions.append(exc_val)
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[exc_val],
                    result=MoltValue("none"),
                )
            )
            self._emit_guarded_body(handler.body, exc_val)
            self.active_exceptions.pop()
            if len(handlers) > 1:
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                emit_handlers(handlers[1:])
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        if node.handlers:
            emit_handlers(node.handlers)

        if node.finalbody:
            final_exc = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=final_exc))
            self.active_exceptions.append(final_exc)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[final_exc],
                    result=MoltValue("none"),
                )
            )
            self._emit_finalbody(node.finalbody, final_exc)
            self.active_exceptions.pop()

        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        if node.orelse:
            self._emit_guarded_body(node.orelse, None)
        if node.finalbody:
            self._emit_finalbody(node.finalbody, None)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.try_suppress_depth = prior_suppress
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True)
        self.try_scopes.pop()
        return None

    def visit_BoolOp(self, node: ast.BoolOp) -> Any:
        if not node.values:
            raise NotImplementedError("Empty bool op is not supported")
        result = self.visit(node.values[0])
        if result is None:
            raise NotImplementedError("Unsupported bool op operand")
        use_phi = self.enable_phi and not self.is_async()
        for value in node.values[1:]:
            if isinstance(node.op, ast.And):
                if use_phi:
                    spill_slot = None
                    if self._expr_may_yield(value):
                        spill_slot = self._spill_async_value(
                            result, f"__bool_left_{len(self.async_locals)}"
                        )
                    self.emit(
                        MoltOp(kind="IF", args=[result], result=MoltValue("none"))
                    )
                    right = self.visit(value)
                    if right is None:
                        raise NotImplementedError("Unsupported bool op operand")
                    self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    left_for_phi = result
                    if spill_slot is not None:
                        left_for_phi = self._reload_async_value(
                            spill_slot, result.type_hint
                        )
                    merged = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="PHI", args=[right, left_for_phi], result=merged)
                    )
                    result = merged
                else:
                    cell = MoltValue(self.next_var(), type_hint="list")
                    self.emit(MoltOp(kind="LIST_NEW", args=[result], result=cell))
                    idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=idx))
                    cell_slot = None
                    idx_slot = None
                    if self._expr_may_yield(value):
                        cell_slot = self._spill_async_value(
                            cell, f"__bool_cell_{len(self.async_locals)}"
                        )
                        idx_slot = self._spill_async_value(
                            idx, f"__bool_idx_{len(self.async_locals)}"
                        )
                    self.emit(
                        MoltOp(kind="IF", args=[result], result=MoltValue("none"))
                    )
                    right = self.visit(value)
                    if right is None:
                        raise NotImplementedError("Unsupported bool op operand")
                    store_cell = cell
                    store_idx = idx
                    if cell_slot is not None and idx_slot is not None:
                        store_cell = self._reload_async_value(cell_slot, "list")
                        store_idx = self._reload_async_value(idx_slot, "int")
                    self.emit(
                        MoltOp(
                            kind="STORE_INDEX",
                            args=[store_cell, store_idx, right],
                            result=MoltValue("none"),
                        )
                    )
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    final_cell = cell
                    final_idx = idx
                    if cell_slot is not None and idx_slot is not None:
                        final_cell = self._reload_async_value(cell_slot, "list")
                        final_idx = self._reload_async_value(idx_slot, "int")
                    result = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="INDEX",
                            args=[final_cell, final_idx],
                            result=result,
                        )
                    )
            elif isinstance(node.op, ast.Or):
                if use_phi:
                    spill_slot = None
                    if self._expr_may_yield(value):
                        spill_slot = self._spill_async_value(
                            result, f"__bool_left_{len(self.async_locals)}"
                        )
                    self.emit(
                        MoltOp(kind="IF", args=[result], result=MoltValue("none"))
                    )
                    self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                    right = self.visit(value)
                    if right is None:
                        raise NotImplementedError("Unsupported bool op operand")
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    left_for_phi = result
                    if spill_slot is not None:
                        left_for_phi = self._reload_async_value(
                            spill_slot, result.type_hint
                        )
                    merged = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="PHI", args=[left_for_phi, right], result=merged)
                    )
                    result = merged
                else:
                    cell = MoltValue(self.next_var(), type_hint="list")
                    self.emit(MoltOp(kind="LIST_NEW", args=[result], result=cell))
                    idx = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=idx))
                    cell_slot = None
                    idx_slot = None
                    if self._expr_may_yield(value):
                        cell_slot = self._spill_async_value(
                            cell, f"__bool_cell_{len(self.async_locals)}"
                        )
                        idx_slot = self._spill_async_value(
                            idx, f"__bool_idx_{len(self.async_locals)}"
                        )
                    self.emit(
                        MoltOp(kind="IF", args=[result], result=MoltValue("none"))
                    )
                    self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                    right = self.visit(value)
                    if right is None:
                        raise NotImplementedError("Unsupported bool op operand")
                    store_cell = cell
                    store_idx = idx
                    if cell_slot is not None and idx_slot is not None:
                        store_cell = self._reload_async_value(cell_slot, "list")
                        store_idx = self._reload_async_value(idx_slot, "int")
                    self.emit(
                        MoltOp(
                            kind="STORE_INDEX",
                            args=[store_cell, store_idx, right],
                            result=MoltValue("none"),
                        )
                    )
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                    final_cell = cell
                    final_idx = idx
                    if cell_slot is not None and idx_slot is not None:
                        final_cell = self._reload_async_value(cell_slot, "list")
                        final_idx = self._reload_async_value(idx_slot, "int")
                    result = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="INDEX",
                            args=[final_cell, final_idx],
                            result=result,
                        )
                    )
            else:
                raise NotImplementedError("Unsupported boolean operator")
        return result

    def visit_Raise(self, node: ast.Raise) -> None:
        def emit_exception_value(
            expr: ast.expr, *, allow_none: bool, context: str
        ) -> MoltValue | None:
            if allow_none and isinstance(expr, ast.Constant) and expr.value is None:
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                return none_val
            if isinstance(expr, ast.Name):
                local = self._load_local_value(expr.id)
                if local is not None:
                    return local
                global_val = self.globals.get(expr.id)
                if global_val is not None:
                    if self.current_func_name == "molt_main":
                        return global_val
                    return self._emit_module_attr_get(expr.id)
                return self._emit_exception_new(expr.id, "")
            if isinstance(expr, ast.Call) and isinstance(expr.func, ast.Name):
                if expr.keywords or len(expr.args) > 1:
                    self._bridge_fallback(
                        node,
                        f"{context} with multiple args/keywords",
                        impact="high",
                        alternative=f"{context} with a single string message",
                        detail="only one positional message is supported",
                    )
                    return None
                msg = ""
                if expr.args:
                    arg = expr.args[0]
                    if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
                        msg = arg.value
                    else:
                        self._bridge_fallback(
                            node,
                            f"{context} with non-string message",
                            impact="high",
                            alternative=f"{context} with a string literal message",
                            detail="non-string messages are not supported yet",
                        )
                        return None
                return self._emit_exception_new(expr.func.id, msg)

            exc_val = self.visit(expr)
            if exc_val is None:
                self._bridge_fallback(
                    node,
                    f"{context} (unsupported expression)",
                    impact="high",
                    alternative=f"{context} a named exception with a string literal",
                    detail="unsupported raise expression form",
                )
                return None
            return exc_val

        if node.exc is None:
            if self.active_exceptions:
                exc_val = self.active_exceptions[-1]
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                is_none = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
                self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
                err_val = self._emit_exception_new(
                    "RuntimeError", "No active exception to reraise"
                )
                self.emit(
                    MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                self._emit_raise_exit()
                return None
            exc_val = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            err_val = self._emit_exception_new(
                "RuntimeError", "No active exception to reraise"
            )
            self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self._emit_raise_exit()
            return None

        exc_val = emit_exception_value(node.exc, allow_none=False, context="raise")
        if exc_val is None:
            return None
        if self.active_exceptions:
            context_val = self.active_exceptions[-1]
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[exc_val, "__context__", context_val],
                    result=MoltValue("none"),
                )
            )
        if node.cause is not None:
            cause_val = emit_exception_value(
                node.cause, allow_none=True, context="raise cause"
            )
            if cause_val is None:
                return None
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_CAUSE",
                    args=[exc_val, cause_val],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self._emit_raise_exit()
        return None

    def visit_Return(self, node: ast.Return) -> None:
        if self.in_generator:
            val = self.visit(node.value) if node.value is not None else None
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            none_exc = None
            max_scopes = len(self.try_scopes) - self.return_unwind_depth
            if max_scopes < 0:
                max_scopes = 0
            if max_scopes > 0:
                if self.context_depth > 0:
                    none_exc = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
                for scope in reversed(self.try_scopes[:max_scopes]):
                    if self.context_depth > 0 and none_exc is not None:
                        ctx_arg = self._ctx_mark_arg(scope)
                        self.emit(
                            MoltOp(
                                kind="CONTEXT_UNWIND_TO",
                                args=[ctx_arg, none_exc],
                                result=MoltValue("none"),
                            )
                        )
                    self.emit(
                        MoltOp(
                            kind="EXCEPTION_POP",
                            args=[],
                            result=MoltValue("none"),
                        )
                    )
                    if scope.finalbody:
                        prior_active = self.active_exceptions[:]
                        self.active_exceptions.clear()
                        self._emit_finalbody(scope.finalbody, None)
                        self.active_exceptions = prior_active
            if self.context_depth > 0:
                if none_exc is None:
                    none_exc = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
                self.emit(
                    MoltOp(
                        kind="CONTEXT_UNWIND",
                        args=[none_exc],
                        result=MoltValue("none"),
                    )
                )
            self._emit_raise_if_pending()
            closed = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", GEN_CLOSED_OFFSET, closed],
                    result=MoltValue("none"),
                )
            )
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=[val, done], result=pair))
            self._emit_return_value(pair)
            return None
        val = self.visit(node.value) if node.value else None
        if val is None:
            val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        none_exc = None
        max_scopes = len(self.try_scopes) - self.return_unwind_depth
        if max_scopes < 0:
            max_scopes = 0
        if max_scopes > 0:
            if self.context_depth > 0:
                none_exc = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
            for scope in reversed(self.try_scopes[:max_scopes]):
                if self.context_depth > 0 and none_exc is not None:
                    ctx_arg = self._ctx_mark_arg(scope)
                    self.emit(
                        MoltOp(
                            kind="CONTEXT_UNWIND_TO",
                            args=[ctx_arg, none_exc],
                            result=MoltValue("none"),
                        )
                    )
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_POP",
                        args=[],
                        result=MoltValue("none"),
                    )
                )
                if scope.finalbody:
                    prior_active = self.active_exceptions[:]
                    self.active_exceptions.clear()
                    self._emit_finalbody(scope.finalbody, None)
                    self.active_exceptions = prior_active
        if self.context_depth > 0:
            if none_exc is None:
                none_exc = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
            self.emit(
                MoltOp(
                    kind="CONTEXT_UNWIND",
                    args=[none_exc],
                    result=MoltValue("none"),
                )
            )
        self._emit_raise_if_pending()
        self._emit_return_value(val)
        return None

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        if node.decorator_list:
            if any(
                self._is_contextmanager_decorator(deco) for deco in node.decorator_list
            ):
                issue = self.compat.bridge_unavailable(
                    node,
                    "contextlib.contextmanager",
                    impact="high",
                    alternative="use explicit context manager objects",
                    detail="generator-based context managers are not lowered yet",
                )
                if self.fallback_policy != "bridge":
                    raise self.compat.error(issue)
                func_name = node.name
                func_symbol = self._function_symbol(func_name)
                prev_func = self.current_func_name
                params = [arg.arg for arg in node.args.args]
                self.globals[func_name] = MoltValue(
                    func_name, type_hint=f"Func:{func_symbol}"
                )
                prev_state = self._capture_function_state()
                self.start_function(
                    func_symbol, params=params, type_facts_name=func_name
                )
                msg_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR",
                        args=[issue.runtime_message()],
                        result=msg_val,
                    )
                )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="BRIDGE_UNAVAILABLE", args=[msg_val], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
                self.resume_function(prev_func)
                self._restore_function_state(prev_state)
                return None
            raise NotImplementedError("Function decorators are not supported yet")
        func_name = node.name
        func_symbol = self._function_symbol(func_name)
        poll_func_name = f"{func_symbol}_poll"
        prev_func = self.current_func_name
        has_return = self._function_contains_return(node)
        params = [arg.arg for arg in node.args.args]

        # Add to globals to support calls from other scopes
        self.globals[func_name] = MoltValue(
            func_name, type_hint=f"AsyncFunc:{poll_func_name}:0"
        )  # Placeholder size

        prev_state = self._capture_function_state()
        self.start_function(
            poll_func_name,
            params=["self"],
            type_facts_name=func_name,
            needs_return_slot=has_return,
        )
        self.global_decls = self._collect_global_decls(node.body)
        for i, arg in enumerate(node.args.args):
            self.async_locals[arg.arg] = self.async_locals_base + i * 8
            if self.type_hint_policy in {"trust", "check"}:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
                if hint is not None:
                    self.async_local_hints[arg.arg] = hint
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        if self.type_hint_policy == "check":
            for arg in node.args.args:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
        for item in node.body:
            self.visit(item)
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self._emit_return_value(res)
            self._emit_return_label()
        else:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        closure_size = self.async_locals_base + len(self.async_locals) * 8
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        func_val = MoltValue(
            self.next_var(), type_hint=f"AsyncFunc:{poll_func_name}:{closure_size}"
        )
        self.emit(
            MoltOp(kind="FUNC_NEW", args=[func_symbol, len(params)], result=func_val)
        )
        self._emit_function_metadata(
            func_val,
            name=func_name,
            qualname=func_name,
            params=params,
            default_exprs=node.args.defaults,
            docstring=ast.get_docstring(node),
            is_coroutine=True,
        )
        if self.current_func_name == "molt_main":
            self.globals[func_name] = func_val
        else:
            self.locals[func_name] = func_val
        self._emit_module_attr_set(func_name, func_val)

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        self.start_function(
            func_symbol,
            params=params,
            type_facts_name=func_name,
        )
        for idx, arg in enumerate(node.args.args):
            hint = None
            if idx == 0 and arg.arg == "self":
                hint = None
            if self.type_hint_policy in {"trust", "check"}:
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
                elif hint is None:
                    hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "int")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in node.args.args:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        args = [self.locals[arg.arg] for arg in node.args.args]
        res = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(
                kind="ALLOC_FUTURE",
                args=[poll_func_name, closure_size] + args,
                result=res,
            )
        )
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        return None

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        is_generator = self._function_contains_yield(node)
        has_return = self._function_contains_return(node)
        if is_generator:
            if node.decorator_list:
                raise NotImplementedError("Function decorators are not supported yet")
            func_name = node.name
            func_symbol = self._function_symbol(func_name)
            poll_func_name = f"{func_symbol}_poll"
            prev_func = self.current_func_name
            params = [arg.arg for arg in node.args.args]

            func_val = MoltValue(
                self.next_var(), type_hint=f"GenFunc:{poll_func_name}:0"
            )
            self.emit(
                MoltOp(
                    kind="FUNC_NEW",
                    args=[poll_func_name, len(params)],
                    result=func_val,
                )
            )
            self._emit_function_metadata(
                func_val,
                name=func_name,
                qualname=func_name,
                params=params,
                default_exprs=node.args.defaults,
                docstring=ast.get_docstring(node),
                is_generator=True,
            )
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self.locals[func_name] = func_val
            self._emit_module_attr_set(func_name, func_val)

            prev_state = self._capture_function_state()
            self.start_function(
                poll_func_name,
                params=["self"],
                type_facts_name=func_name,
                needs_return_slot=has_return,
            )
            self.global_decls = self._collect_global_decls(node.body)
            self.in_generator = True
            self.async_locals_base = GEN_CONTROL_SIZE
            for i, arg in enumerate(node.args.args):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                if self.type_hint_policy in {"trust", "check"}:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            if self.type_hint_policy == "check":
                for arg in node.args.args:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            for item in node.body:
                self.visit(item)
                if isinstance(item, (ast.Return, ast.Raise)):
                    break
            if self.return_label is not None:
                if not self._ends_with_return_jump():
                    none_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                    closed = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", GEN_CLOSED_OFFSET, closed],
                            result=MoltValue("none"),
                        )
                    )
                    done = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                    pair = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                    )
                    self._emit_return_value(pair)
                self._emit_return_label()
            elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                closed = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=closed))
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", GEN_CLOSED_OFFSET, closed],
                        result=MoltValue("none"),
                    )
                )
                done = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=done))
                pair = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
                self.emit(MoltOp(kind="ret", args=[pair], result=MoltValue("none")))
            closure_size = self.async_locals_base + len(self.async_locals) * 8
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            func_val.type_hint = f"GenFunc:{poll_func_name}:{closure_size}"
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self.locals[func_name] = func_val
            return None

        if node.decorator_list:
            if any(
                self._is_contextmanager_decorator(deco) for deco in node.decorator_list
            ):
                issue = self.compat.bridge_unavailable(
                    node,
                    "contextlib.contextmanager",
                    impact="high",
                    alternative="use explicit context manager objects",
                    detail="generator-based context managers are not lowered yet",
                )
                if self.fallback_policy != "bridge":
                    raise self.compat.error(issue)
                func_name = node.name
                func_symbol = self._function_symbol(func_name)
                prev_func = self.current_func_name
                params = [arg.arg for arg in node.args.args]
                func_val = MoltValue(self.next_var(), type_hint=f"Func:{func_symbol}")
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[func_symbol, len(params)],
                        result=func_val,
                    )
                )
                self._emit_function_metadata(
                    func_val,
                    name=func_name,
                    qualname=func_name,
                    params=params,
                    default_exprs=node.args.defaults,
                    docstring=ast.get_docstring(node),
                )
                if self.current_func_name == "molt_main":
                    self.globals[func_name] = func_val
                else:
                    self.locals[func_name] = func_val
                self._emit_module_attr_set(func_name, func_val)
                prev_state = self._capture_function_state()
                self.start_function(
                    func_symbol, params=params, type_facts_name=func_name
                )
                msg_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(
                        kind="CONST_STR",
                        args=[issue.runtime_message()],
                        result=msg_val,
                    )
                )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="BRIDGE_UNAVAILABLE", args=[msg_val], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
                self.resume_function(prev_func)
                self._restore_function_state(prev_state)
                return None
            raise NotImplementedError("Function decorators are not supported yet")
        func_name = node.name
        func_symbol = self._function_symbol(func_name)
        prev_func = self.current_func_name
        params = [arg.arg for arg in node.args.args]

        func_val = MoltValue(self.next_var(), type_hint=f"Func:{func_symbol}")
        self.emit(
            MoltOp(kind="FUNC_NEW", args=[func_symbol, len(params)], result=func_val)
        )
        self._emit_function_metadata(
            func_val,
            name=func_name,
            qualname=func_name,
            params=params,
            default_exprs=node.args.defaults,
            docstring=ast.get_docstring(node),
        )
        if self.current_func_name == "molt_main":
            self.globals[func_name] = func_val
        else:
            self.locals[func_name] = func_val
        self._emit_module_attr_set(func_name, func_val)

        prev_state = self._capture_function_state()
        self.start_function(
            func_symbol,
            params=params,
            type_facts_name=func_name,
            needs_return_slot=has_return,
        )
        self.global_decls = self._collect_global_decls(node.body)
        for arg in node.args.args:
            hint = None
            if self.type_hint_policy == "ignore" and arg.annotation is not None:
                inferred = self._annotation_to_hint(arg.annotation)
                if inferred is not None and inferred in self.classes:
                    hint = inferred
            if self.type_hint_policy in {"trust", "check"}:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
            if hint is None and self.type_hint_policy in {"trust", "check"}:
                hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "int")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in node.args.args:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        for item in node.body:
            self.visit(item)
        if self.return_label is not None:
            if not self._ends_with_return_jump():
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                self._emit_return_value(res)
            self._emit_return_label()
        elif not (self.current_ops and self.current_ops[-1].kind == "ret"):
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        return None

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            module_name = alias.name
            if module_name in {"typing", "typing_extensions"}:
                continue
            bind_name = alias.asname or module_name.split(".")[0]
            module_val = self._emit_module_load_with_parents(module_name)
            if alias.asname:
                bound_val = module_val
            else:
                top_name = module_name.split(".")[0]
                bound_val = self._emit_module_load(top_name)
            self.locals[bind_name] = bound_val
            self.exact_locals.pop(bind_name, None)
            if self.current_func_name == "molt_main":
                self.globals[bind_name] = bound_val
            self._emit_module_attr_set(bind_name, bound_val)
        return None

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module is None:
            raise NotImplementedError("Relative imports are not supported yet")
        if node.module in {"__future__", "typing", "typing_extensions"}:
            return None
        module_name = node.module
        module_val = self._emit_module_load_with_parents(module_name)
        for alias in node.names:
            if alias.name == "*":
                raise NotImplementedError("import * is not supported yet")
            attr_name = alias.name
            bind_name = alias.asname or attr_name
            submodule_name = f"{module_name}.{attr_name}"
            if (
                self.known_modules
                and module_name == "molt.stdlib"
                and attr_name in self.known_modules
            ):
                attr_val = self._emit_module_load_with_parents(attr_name)
                self._emit_module_attr_set_on(module_val, attr_name, attr_val)
            elif self.known_modules and submodule_name in self.known_modules:
                attr_val = self._emit_module_load_with_parents(submodule_name)
            else:
                attr_val = MoltValue(self.next_var(), type_hint="Any")
                attr_name_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=[attr_name], result=attr_name_val)
                )
                self.emit(
                    MoltOp(
                        kind="MODULE_GET_ATTR",
                        args=[module_val, attr_name_val],
                        result=attr_val,
                    )
                )
            if module_name == "asyncio" and attr_name in {"run", "sleep"}:
                module_prefix = f"{self._sanitize_module_name(module_name)}__"
                attr_val.type_hint = f"Func:{module_prefix}{attr_name}"
            self.locals[bind_name] = attr_val
            self.exact_locals.pop(bind_name, None)
            if self.current_func_name == "molt_main":
                self.globals[bind_name] = attr_val
            self._emit_module_attr_set(bind_name, attr_val)
        return None

    def _emit_await_anext(
        self,
        iter_obj: MoltValue,
        *,
        default_val: MoltValue | None,
        has_default: bool,
    ) -> MoltValue:
        if iter_obj.type_hint in {"iter", "generator"}:
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
            none_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
            is_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[pair, none_val], result=is_none))
            self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
            err_val = self._emit_exception_new("TypeError", "object is not an iterator")
            self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            if has_default:
                if default_val is None:
                    default_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
            else:
                default_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
            res_cell = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[default_val], result=res_cell))
            self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
            if not has_default:
                stop_val = self._emit_exception_new("StopAsyncIteration", "")
                self.emit(
                    MoltOp(kind="RAISE", args=[stop_val], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=val))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res_cell, zero, val],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[res_cell, zero], result=res))
            return res

        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        awaitable_slot = None
        if self.is_async():
            awaitable_slot = self._async_local_offset(
                f"__anext_future_{len(self.async_locals)}"
            )
            awaitable_cached = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", awaitable_slot],
                    result=awaitable_cached,
                )
            )
            none_cached = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_cached))
            is_none_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS",
                    args=[awaitable_cached, none_cached],
                    result=is_none_cached,
                )
            )
            zero_cached = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=zero_cached))
            is_zero_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS",
                    args=[awaitable_cached, zero_cached],
                    result=is_zero_cached,
                )
            )
            is_empty_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="OR",
                    args=[is_none_cached, is_zero_cached],
                    result=is_empty_cached,
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_empty_cached], result=MoltValue("none"))
            )
            awaitable_new = MoltValue(self.next_var(), type_hint="Future")
            self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=awaitable_new))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", awaitable_slot, awaitable_new],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            awaitable = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", awaitable_slot],
                    result=awaitable,
                )
            )
        else:
            awaitable = MoltValue(self.next_var(), type_hint="Future")
            self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=awaitable))
        if has_default:
            if default_val is None:
                default_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        else:
            default_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=default_val))
        res_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[default_val], result=res_cell))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        cell_slot: int | None = None
        if self.is_async():
            cell_slot = self._async_local_offset(
                f"__anext_cell_{len(self.async_locals)}"
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", cell_slot, res_cell],
                    result=MoltValue("none"),
                )
            )
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_val))
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_val, none_val], result=is_none))
        pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=pending))
        self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="EXCEPTION_KIND", args=[exc_val], result=kind_val))
        stop_kind = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(kind="CONST_STR", args=["StopAsyncIteration"], result=stop_kind)
        )
        is_stop = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[kind_val, stop_kind], result=is_stop))
        self.emit(MoltOp(kind="IF", args=[is_stop], result=MoltValue("none")))
        if not has_default:
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        else:
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.state_count += 1
        pending_state_id = self.state_count
        self.emit(
            MoltOp(
                kind="STATE_LABEL", args=[pending_state_id], result=MoltValue("none")
            )
        )
        pending_state_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(kind="CONST", args=[pending_state_id], result=pending_state_val)
        )
        awaitable_for_poll = awaitable
        if awaitable_slot is not None:
            awaitable_for_poll = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", awaitable_slot],
                    result=awaitable_for_poll,
                )
            )
        self.state_count += 1
        next_state_id = self.state_count
        await_result_slot = self._async_local_offset(
            f"__anext_result_{len(self.async_locals)}"
        )
        await_slot_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[await_result_slot], result=await_slot_val))
        awaited = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="STATE_TRANSITION",
                args=[
                    awaitable_for_poll,
                    await_slot_val,
                    pending_state_val,
                    next_state_id,
                ],
                result=awaited,
            )
        )
        if awaitable_slot is not None:
            cleared_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", awaitable_slot, cleared_val],
                    result=MoltValue("none"),
                )
            )
        awaited_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", await_result_slot],
                result=awaited_val,
            )
        )
        exc_after = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
        none_after = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
        is_none_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after))
        pending_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none_after], result=pending_after))
        self.emit(MoltOp(kind="IF", args=[pending_after], result=MoltValue("none")))
        kind_after = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="EXCEPTION_KIND", args=[exc_after], result=kind_after))
        stop_after = MoltValue(self.next_var(), type_hint="str")
        self.emit(
            MoltOp(kind="CONST_STR", args=["StopAsyncIteration"], result=stop_after)
        )
        is_stop_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="EQ", args=[kind_after, stop_after], result=is_stop_after)
        )
        self.emit(MoltOp(kind="IF", args=[is_stop_after], result=MoltValue("none")))
        if not has_default:
            self.emit(MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none")))
        else:
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="RAISE", args=[exc_after], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
        if cell_slot is not None:
            res_cell_after = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", cell_slot],
                    result=res_cell_after,
                )
            )
            zero_after = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero_after))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res_cell_after, zero_after, awaited_val],
                    result=MoltValue("none"),
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res_cell, zero, awaited_val],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending()
        if cell_slot is not None:
            res_cell_final = MoltValue(self.next_var(), type_hint="list")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", cell_slot],
                    result=res_cell_final,
                )
            )
            zero_final = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero_final))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(kind="INDEX", args=[res_cell_final, zero_final], result=res)
            )
        else:
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[res_cell, zero], result=res))
        return res

    def visit_Await(self, node: ast.Await) -> Any:
        if (
            isinstance(node.value, ast.Call)
            and isinstance(node.value.func, ast.Name)
            and node.value.func.id == "anext"
        ):
            if node.value.keywords or len(node.value.args) not in (1, 2):
                raise NotImplementedError("anext expects 1 or 2 positional arguments")
            iter_obj = self.visit(node.value.args[0])
            if iter_obj is None:
                raise NotImplementedError("Unsupported iterator in anext()")
            has_default = len(node.value.args) == 2
            default_val = self.visit(node.value.args[1]) if has_default else None
            return self._emit_await_anext(
                iter_obj, default_val=default_val, has_default=has_default
            )
        awaitable_slot = None
        if self.is_async():
            awaitable_slot = self._async_local_offset(
                f"__await_future_{len(self.async_locals)}"
            )
            awaitable_cached = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", awaitable_slot],
                    result=awaitable_cached,
                )
            )
            none_cached = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_cached))
            is_none_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS",
                    args=[awaitable_cached, none_cached],
                    result=is_none_cached,
                )
            )
            zero_cached = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=zero_cached))
            is_zero_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS",
                    args=[awaitable_cached, zero_cached],
                    result=is_zero_cached,
                )
            )
            is_empty_cached = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="OR",
                    args=[is_none_cached, is_zero_cached],
                    result=is_empty_cached,
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_empty_cached], result=MoltValue("none"))
            )
            awaitable_new = self.visit(node.value)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", awaitable_slot, awaitable_new],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.state_count += 1
            pending_state_id = self.state_count
            self.emit(
                MoltOp(
                    kind="STATE_LABEL",
                    args=[pending_state_id],
                    result=MoltValue("none"),
                )
            )
            pending_state_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[pending_state_id], result=pending_state_val)
            )
            coro = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", awaitable_slot],
                    result=coro,
                )
            )
        else:
            coro = self.visit(node.value)
        result_slot = self._async_local_offset(
            f"__await_result_{len(self.async_locals)}"
        )
        result_slot_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[result_slot], result=result_slot_val))
        self.state_count += 1
        next_state_id = self.state_count
        res_placeholder = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="STATE_TRANSITION",
                args=[coro, result_slot_val, pending_state_val, next_state_id],
                result=res_placeholder,
            )
        )
        if awaitable_slot is not None:
            cleared_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=cleared_val))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", awaitable_slot, cleared_val],
                    result=MoltValue("none"),
                )
            )
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", result_slot],
                result=res,
            )
        )
        return res

    def visit_Yield(self, node: ast.Yield) -> Any:
        if not self.in_generator:
            raise NotImplementedError("yield outside of generator")
        if node.value is None:
            value = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=value))
        else:
            value = self.visit(node.value)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=done))
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=[value, done], result=pair))
        self.state_count += 1
        self.emit(
            MoltOp(
                kind="STATE_YIELD",
                args=[pair, self.state_count],
                result=MoltValue("none"),
            )
        )
        throw_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_THROW_OFFSET],
                result=throw_val,
            )
        )
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[throw_val, none_val], result=is_none))
        not_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none], result=not_none))
        self.emit(MoltOp(kind="IF", args=[not_none], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_THROW_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="RAISE", args=[throw_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_SEND_OFFSET],
                result=res,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_SEND_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        return res

    def visit_YieldFrom(self, node: ast.YieldFrom) -> Any:
        if not self.in_generator:
            raise NotImplementedError("yield from outside of generator")
        iterable = self.visit(node.value)
        if iterable is None:
            raise NotImplementedError("yield from operand unsupported")
        iter_obj = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=iter_obj))
        is_gen = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS_GENERATOR", args=[iter_obj], result=is_gen))
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        iter_slot = None
        is_gen_slot = None
        pair_slot = None
        if self.is_async():
            iter_slot = self._async_local_offset(f"__yf_iter_{len(self.async_locals)}")
            is_gen_slot = self._async_local_offset(
                f"__yf_is_gen_{len(self.async_locals)}"
            )
            pair_slot = self._async_local_offset(f"__yf_pair_{len(self.async_locals)}")
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", iter_slot, iter_obj],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", is_gen_slot, is_gen],
                    result=MoltValue("none"),
                )
            )
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )

        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        if iter_slot is not None:
            iter_obj = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", iter_slot],
                    result=iter_obj,
                )
            )
            is_gen = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", is_gen_slot],
                    result=is_gen,
                )
            )
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", pair_slot],
                    result=pair,
                )
            )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        value = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=value))
        yielded = MoltValue(self.next_var(), type_hint="tuple")
        done_false = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=done_false))
        self.emit(MoltOp(kind="TUPLE_NEW", args=[value, done_false], result=yielded))
        self.state_count += 1
        self.emit(
            MoltOp(
                kind="STATE_YIELD",
                args=[yielded, self.state_count],
                result=MoltValue("none"),
            )
        )
        if iter_slot is not None:
            iter_obj = MoltValue(self.next_var(), type_hint="iter")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", iter_slot],
                    result=iter_obj,
                )
            )
            is_gen = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", is_gen_slot],
                    result=is_gen,
                )
            )
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", pair_slot],
                    result=pair,
                )
            )
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        pending_throw = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_THROW_OFFSET],
                result=pending_throw,
            )
        )
        throw_is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="IS", args=[pending_throw, none_val], result=throw_is_none)
        )
        throw_pending = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[throw_is_none], result=throw_pending))
        self.emit(MoltOp(kind="IF", args=[throw_pending], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_THROW_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="IF", args=[is_gen], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="GEN_THROW",
                args=[iter_obj, pending_throw],
                result=pair,
            )
        )
        if pair_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="RAISE", args=[pending_throw], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        pending_send = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", GEN_SEND_OFFSET],
                result=pending_send,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", GEN_SEND_OFFSET, none_val],
                result=MoltValue("none"),
            )
        )
        send_is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[pending_send, none_val], result=send_is_none))
        self.emit(MoltOp(kind="IF", args=[send_is_none], result=MoltValue("none")))
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        if pair_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_gen], result=MoltValue("none")))
        self.emit(MoltOp(kind="GEN_SEND", args=[iter_obj, pending_send], result=pair))
        if pair_slot is not None:
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", pair_slot, pair],
                    result=MoltValue("none"),
                )
            )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        err_val = self._emit_exception_new(
            "TypeError", "can't send non-None to a non-generator iterator"
        )
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        if pair_slot is not None:
            pair = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", pair_slot],
                    result=pair,
                )
            )
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        result = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=result))
        return result

    def map_ops_to_json(self, ops: list[MoltOp]) -> list[dict[str, Any]]:
        json_ops: list[dict[str, Any]] = []
        for op in ops:
            if op.kind == "CONST":
                value = op.args[0]
                if isinstance(value, bool):
                    value = 1 if value else 0
                json_ops.append(
                    {"kind": "const", "value": value, "out": op.result.name}
                )
            elif op.kind == "CONST_BOOL":
                value = 1 if op.args[0] else 0
                json_ops.append(
                    {"kind": "const_bool", "value": value, "out": op.result.name}
                )
            elif op.kind == "CONST_FLOAT":
                json_ops.append(
                    {
                        "kind": "const_float",
                        "f_value": op.args[0],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONST_STR":
                json_ops.append(
                    {"kind": "const_str", "s_value": op.args[0], "out": op.result.name}
                )
            elif op.kind == "CONST_BYTES":
                json_ops.append(
                    {
                        "kind": "const_bytes",
                        "bytes": list(op.args[0]),
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONST_NONE":
                json_ops.append({"kind": "const_none", "out": op.result.name})
            elif op.kind == "ADD":
                add_entry: dict[str, Any] = {
                    "kind": "add",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    add_entry["fast_int"] = True
                json_ops.append(add_entry)
            elif op.kind == "SUB":
                sub_entry: dict[str, Any] = {
                    "kind": "sub",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    sub_entry["fast_int"] = True
                json_ops.append(sub_entry)
            elif op.kind == "MUL":
                mul_entry: dict[str, Any] = {
                    "kind": "mul",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    mul_entry["fast_int"] = True
                json_ops.append(mul_entry)
            elif op.kind == "LT":
                lt_entry: dict[str, Any] = {
                    "kind": "lt",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    lt_entry["fast_int"] = True
                json_ops.append(lt_entry)
            elif op.kind == "EQ":
                eq_entry: dict[str, Any] = {
                    "kind": "eq",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    eq_entry["fast_int"] = True
                json_ops.append(eq_entry)
            elif op.kind == "IS":
                json_ops.append(
                    {
                        "kind": "is",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "NOT":
                json_ops.append(
                    {
                        "kind": "not",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "AND":
                json_ops.append(
                    {
                        "kind": "and",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "OR":
                json_ops.append(
                    {
                        "kind": "or",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTAINS":
                json_ops.append(
                    {
                        "kind": "contains",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IF":
                json_ops.append({"kind": "if", "args": [op.args[0].name]})
            elif op.kind == "ELSE":
                json_ops.append({"kind": "else"})
            elif op.kind == "END_IF":
                json_ops.append({"kind": "end_if"})
            elif op.kind == "CALL":
                target = op.args[0]
                json_ops.append(
                    {
                        "kind": "call",
                        "s_value": target,
                        "args": [arg.name for arg in op.args[1:]],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_GUARDED":
                target = op.metadata["target"] if op.metadata else ""
                json_ops.append(
                    {
                        "kind": "call_guarded",
                        "s_value": target,
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_FUNC":
                json_ops.append(
                    {
                        "kind": "call_func",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_METHOD":
                json_ops.append(
                    {
                        "kind": "call_method",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUNC_NEW":
                func_name, arity = op.args
                json_ops.append(
                    {
                        "kind": "func_new",
                        "s_value": func_name,
                        "value": arity,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_NEW":
                json_ops.append(
                    {
                        "kind": "class_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_SET_BASE":
                json_ops.append(
                    {
                        "kind": "class_set_base",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_APPLY_SET_NAME":
                json_ops.append(
                    {
                        "kind": "class_apply_set_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SUPER_NEW":
                json_ops.append(
                    {
                        "kind": "super_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUILTIN_TYPE":
                json_ops.append(
                    {
                        "kind": "builtin_type",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TYPE_OF":
                json_ops.append(
                    {
                        "kind": "type_of",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ISINSTANCE":
                json_ops.append(
                    {
                        "kind": "isinstance",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ISSUBCLASS":
                json_ops.append(
                    {
                        "kind": "issubclass",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "OBJECT_NEW":
                json_ops.append(
                    {"kind": "object_new", "args": [], "out": op.result.name}
                )
            elif op.kind == "CLASSMETHOD_NEW":
                json_ops.append(
                    {
                        "kind": "classmethod_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATICMETHOD_NEW":
                json_ops.append(
                    {
                        "kind": "staticmethod_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROPERTY_NEW":
                json_ops.append(
                    {
                        "kind": "property_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BOUND_METHOD_NEW":
                json_ops.append(
                    {
                        "kind": "bound_method_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_NEW":
                json_ops.append(
                    {
                        "kind": "module_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_CACHE_GET":
                json_ops.append(
                    {
                        "kind": "module_cache_get",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_CACHE_SET":
                json_ops.append(
                    {
                        "kind": "module_cache_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_GET_ATTR":
                json_ops.append(
                    {
                        "kind": "module_get_attr",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_SET_ATTR":
                json_ops.append(
                    {
                        "kind": "module_set_attr",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_NULL":
                json_ops.append(
                    {
                        "kind": "context_null",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_ENTER":
                json_ops.append(
                    {
                        "kind": "context_enter",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_EXIT":
                json_ops.append(
                    {
                        "kind": "context_exit",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_UNWIND":
                json_ops.append(
                    {
                        "kind": "context_unwind",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_DEPTH":
                json_ops.append({"kind": "context_depth", "out": op.result.name})
            elif op.kind == "CONTEXT_UNWIND_TO":
                json_ops.append(
                    {
                        "kind": "context_unwind_to",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CONTEXT_CLOSING":
                json_ops.append(
                    {
                        "kind": "context_closing",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_PUSH":
                json_ops.append({"kind": "exception_push", "out": op.result.name})
            elif op.kind == "EXCEPTION_POP":
                json_ops.append({"kind": "exception_pop", "out": op.result.name})
            elif op.kind == "EXCEPTION_LAST":
                json_ops.append({"kind": "exception_last", "out": op.result.name})
            elif op.kind == "EXCEPTION_NEW":
                json_ops.append(
                    {
                        "kind": "exception_new",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_SET_CAUSE":
                json_ops.append(
                    {
                        "kind": "exception_set_cause",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_CONTEXT_SET":
                json_ops.append(
                    {
                        "kind": "exception_context_set",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_CLEAR":
                json_ops.append({"kind": "exception_clear", "out": op.result.name})
            elif op.kind == "EXCEPTION_KIND":
                json_ops.append(
                    {
                        "kind": "exception_kind",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EXCEPTION_MESSAGE":
                json_ops.append(
                    {
                        "kind": "exception_message",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "RAISE":
                json_ops.append(
                    {
                        "kind": "raise",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TRY_START":
                json_ops.append({"kind": "try_start"})
            elif op.kind == "TRY_END":
                json_ops.append({"kind": "try_end"})
            elif op.kind == "LABEL":
                json_ops.append({"kind": "label", "value": op.args[0]})
            elif op.kind == "STATE_LABEL":
                json_ops.append({"kind": "state_label", "value": op.args[0]})
            elif op.kind == "JUMP":
                json_ops.append({"kind": "jump", "value": op.args[0]})
            elif op.kind == "PHI":
                json_ops.append(
                    {
                        "kind": "phi",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHECK_EXCEPTION":
                json_ops.append({"kind": "check_exception", "value": op.args[0]})
            elif op.kind == "FILE_OPEN":
                json_ops.append(
                    {
                        "kind": "file_open",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FILE_READ":
                json_ops.append(
                    {
                        "kind": "file_read",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FILE_WRITE":
                json_ops.append(
                    {
                        "kind": "file_write",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FILE_CLOSE":
                json_ops.append(
                    {
                        "kind": "file_close",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ENV_GET":
                json_ops.append(
                    {
                        "kind": "env_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PRINT":
                json_ops.append(
                    {
                        "kind": "print",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                    }
                )
            elif op.kind == "PRINT_NEWLINE":
                json_ops.append({"kind": "print_newline"})
            elif op.kind == "ALLOC":
                json_ops.append(
                    {
                        "kind": "alloc",
                        "value": self.classes[op.args[0]]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "OBJECT_SET_CLASS":
                json_ops.append(
                    {
                        "kind": "object_set_class",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_NEW":
                json_ops.append(
                    {
                        "kind": "dataclass_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SETATTR":
                obj, attr, val, *rest = op.args
                if rest:
                    expected_class = rest[0]
                else:
                    expected_class = list(self.classes.keys())[-1]
                offset = self.classes[expected_class]["fields"][attr]
                json_ops.append(
                    {"kind": "store", "args": [obj.name, val.name], "value": offset}
                )
            elif op.kind == "SETATTR_GENERIC_PTR":
                json_ops.append(
                    {
                        "kind": "set_attr_generic_ptr",
                        "args": [op.args[0].name, op.args[2].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SETATTR_GENERIC_OBJ":
                json_ops.append(
                    {
                        "kind": "set_attr_generic_obj",
                        "args": [op.args[0].name, op.args[2].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DELATTR_GENERIC_PTR":
                json_ops.append(
                    {
                        "kind": "del_attr_generic_ptr",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DELATTR_GENERIC_OBJ":
                json_ops.append(
                    {
                        "kind": "del_attr_generic_obj",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_GET":
                json_ops.append(
                    {
                        "kind": "dataclass_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_SET":
                json_ops.append(
                    {
                        "kind": "dataclass_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DATACLASS_SET_CLASS":
                json_ops.append(
                    {
                        "kind": "dataclass_set_class",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR":
                obj, attr = op.args
                offset = self.classes[list(self.classes.keys())[-1]]["fields"][attr]
                json_ops.append(
                    {
                        "kind": "load",
                        "args": [obj.name],
                        "value": offset,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GUARDED_GETATTR":
                obj, attr, expected_class = op.args
                offset = self.classes[expected_class]["fields"][attr]
                json_ops.append(
                    {
                        "kind": "guarded_load",
                        "args": [obj.name],
                        "s_value": attr,
                        "value": offset,
                        "out": op.result.name,
                        "metadata": {"expected_type_id": 100},
                    }
                )
            elif op.kind == "GETATTR_GENERIC_PTR":
                json_ops.append(
                    {
                        "kind": "get_attr_generic_ptr",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR_GENERIC_OBJ":
                json_ops.append(
                    {
                        "kind": "get_attr_generic_obj",
                        "args": [op.args[0].name],
                        "s_value": op.args[1],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR_NAME":
                json_ops.append(
                    {
                        "kind": "get_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GETATTR_NAME_DEFAULT":
                json_ops.append(
                    {
                        "kind": "get_attr_name_default",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "HASATTR_NAME":
                json_ops.append(
                    {
                        "kind": "has_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SETATTR_NAME":
                json_ops.append(
                    {
                        "kind": "set_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DELATTR_NAME":
                json_ops.append(
                    {
                        "kind": "del_attr_name",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GUARD_TYPE":
                json_ops.append(
                    {
                        "kind": "guard_type",
                        "args": [arg.name for arg in op.args],
                    }
                )
            elif op.kind == "JSON_PARSE":
                json_ops.append(
                    {
                        "kind": "json_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MSGPACK_PARSE":
                json_ops.append(
                    {
                        "kind": "msgpack_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CBOR_PARSE":
                json_ops.append(
                    {
                        "kind": "cbor_parse",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LEN":
                json_ops.append(
                    {
                        "kind": "len",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_NEW":
                json_ops.append(
                    {
                        "kind": "list_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "RANGE_NEW":
                json_ops.append(
                    {
                        "kind": "range_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_NEW":
                json_ops.append(
                    {
                        "kind": "tuple_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_APPEND":
                json_ops.append(
                    {
                        "kind": "list_append",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_POP":
                json_ops.append(
                    {
                        "kind": "list_pop",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_EXTEND":
                json_ops.append(
                    {
                        "kind": "list_extend",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_INSERT":
                json_ops.append(
                    {
                        "kind": "list_insert",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_REMOVE":
                json_ops.append(
                    {
                        "kind": "list_remove",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_COUNT":
                json_ops.append(
                    {
                        "kind": "list_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_INDEX":
                json_ops.append(
                    {
                        "kind": "list_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_FROM_LIST":
                json_ops.append(
                    {
                        "kind": "tuple_from_list",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "bytes_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "bytearray_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INTARRAY_FROM_SEQ":
                json_ops.append(
                    {
                        "kind": "intarray_from_seq",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MEMORYVIEW_NEW":
                json_ops.append(
                    {
                        "kind": "memoryview_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MEMORYVIEW_TOBYTES":
                json_ops.append(
                    {
                        "kind": "memoryview_tobytes",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_NEW":
                json_ops.append(
                    {
                        "kind": "dict_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_GET":
                json_ops.append(
                    {
                        "kind": "dict_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_POP":
                json_ops.append(
                    {
                        "kind": "dict_pop",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_KEYS":
                json_ops.append(
                    {
                        "kind": "dict_keys",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_VALUES":
                json_ops.append(
                    {
                        "kind": "dict_values",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_ITEMS":
                json_ops.append(
                    {
                        "kind": "dict_items",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_COUNT":
                json_ops.append(
                    {
                        "kind": "tuple_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TUPLE_INDEX":
                json_ops.append(
                    {
                        "kind": "tuple_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ITER_NEW":
                json_ops.append(
                    {
                        "kind": "iter",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "AITER":
                json_ops.append(
                    {
                        "kind": "aiter",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ITER_NEXT":
                json_ops.append(
                    {
                        "kind": "iter_next",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ANEXT":
                json_ops.append(
                    {
                        "kind": "anext",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INDEX":
                json_ops.append(
                    {
                        "kind": "index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STORE_INDEX":
                json_ops.append(
                    {
                        "kind": "store_index",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_START":
                json_ops.append({"kind": "loop_start"})
            elif op.kind == "LOOP_INDEX_START":
                json_ops.append(
                    {
                        "kind": "loop_index_start",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_INDEX_NEXT":
                json_ops.append(
                    {
                        "kind": "loop_index_next",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOOP_BREAK_IF_TRUE":
                json_ops.append(
                    {"kind": "loop_break_if_true", "args": [op.args[0].name]}
                )
            elif op.kind == "LOOP_BREAK_IF_FALSE":
                json_ops.append(
                    {"kind": "loop_break_if_false", "args": [op.args[0].name]}
                )
            elif op.kind == "LOOP_CONTINUE":
                json_ops.append({"kind": "loop_continue"})
            elif op.kind == "LOOP_END":
                json_ops.append({"kind": "loop_end"})
            elif op.kind == "VEC_SUM_INT":
                json_ops.append(
                    {
                        "kind": "vec_sum_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_SUM_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_sum_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT":
                json_ops.append(
                    {
                        "kind": "vec_prod_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_prod_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_prod_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_PROD_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_prod_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT":
                json_ops.append(
                    {
                        "kind": "vec_min_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_min_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_min_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MIN_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_min_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT":
                json_ops.append(
                    {
                        "kind": "vec_max_int",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_max_int_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT_RANGE":
                json_ops.append(
                    {
                        "kind": "vec_max_int_range",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "VEC_MAX_INT_RANGE_TRUSTED":
                json_ops.append(
                    {
                        "kind": "vec_max_int_range_trusted",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SLICE":
                json_ops.append(
                    {
                        "kind": "slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SLICE_NEW":
                json_ops.append(
                    {
                        "kind": "slice_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_FIND":
                json_ops.append(
                    {
                        "kind": "bytes_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_FIND":
                json_ops.append(
                    {
                        "kind": "bytearray_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STR_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "str_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_FIND":
                json_ops.append(
                    {
                        "kind": "string_find",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_FORMAT":
                json_ops.append(
                    {
                        "kind": "string_format",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_NEW":
                json_ops.append(
                    {
                        "kind": "buffer2d_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_GET":
                json_ops.append(
                    {
                        "kind": "buffer2d_get",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_SET":
                json_ops.append(
                    {
                        "kind": "buffer2d_set",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BUFFER2D_MATMUL":
                json_ops.append(
                    {
                        "kind": "buffer2d_matmul",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_STARTSWITH":
                json_ops.append(
                    {
                        "kind": "string_startswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_ENDSWITH":
                json_ops.append(
                    {
                        "kind": "string_endswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_COUNT":
                json_ops.append(
                    {
                        "kind": "string_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_JOIN":
                json_ops.append(
                    {
                        "kind": "string_join",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_SPLIT":
                json_ops.append(
                    {
                        "kind": "string_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_REPLACE":
                json_ops.append(
                    {
                        "kind": "string_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_SPLIT":
                json_ops.append(
                    {
                        "kind": "bytes_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_SPLIT":
                json_ops.append(
                    {
                        "kind": "bytearray_split",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_REPLACE":
                json_ops.append(
                    {
                        "kind": "bytes_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_REPLACE":
                json_ops.append(
                    {
                        "kind": "bytearray_replace",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASYNC_BLOCK_ON":
                json_ops.append(
                    {
                        "kind": "block_on",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_DUMMY":
                json_ops.append({"kind": "const", "value": 0, "out": op.result.name})
            elif op.kind == "BRIDGE_UNAVAILABLE":
                json_ops.append(
                    {
                        "kind": "bridge_unavailable",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ret":
                json_ops.append({"kind": "ret", "var": op.args[0].name})
            elif op.kind == "ALLOC_FUTURE":
                poll_func = op.args[0]
                size = op.args[1]
                args = op.args[2:]
                json_ops.append(
                    {
                        "kind": "alloc_future",
                        "s_value": poll_func,
                        "value": size,
                        "args": [arg.name for arg in args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ALLOC_GENERATOR":
                poll_func = op.args[0]
                size = op.args[1]
                args = op.args[2:]
                json_ops.append(
                    {
                        "kind": "alloc_generator",
                        "s_value": poll_func,
                        "value": size,
                        "args": [arg.name for arg in args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATE_SWITCH":
                json_ops.append({"kind": "state_switch"})
            elif op.kind == "STATE_TRANSITION":
                if len(op.args) == 3:
                    future, pending_state, next_state = op.args
                    slot_arg = None
                else:
                    future, slot_arg, pending_state, next_state = op.args
                args = [future.name]
                if slot_arg is not None:
                    args.append(slot_arg.name)
                args.append(pending_state.name)
                json_ops.append(
                    {
                        "kind": "state_transition",
                        "args": args,
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STATE_YIELD":
                pair, next_state = op.args
                json_ops.append(
                    {
                        "kind": "state_yield",
                        "args": [pair.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SPAWN":
                json_ops.append({"kind": "spawn", "args": [op.args[0].name]})
            elif op.kind == "CHAN_NEW":
                json_ops.append(
                    {
                        "kind": "chan_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHAN_SEND_YIELD":
                chan, val, pending_state, next_state = op.args
                json_ops.append(
                    {
                        "kind": "chan_send_yield",
                        "args": [chan.name, val.name, pending_state.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHAN_RECV_YIELD":
                chan, pending_state, next_state = op.args
                json_ops.append(
                    {
                        "kind": "chan_recv_yield",
                        "args": [chan.name, pending_state.name],
                        "value": next_state,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALL_ASYNC":
                json_ops.append(
                    {"kind": "call_async", "s_value": op.args[0], "out": op.result.name}
                )
            elif op.kind == "GEN_SEND":
                gen, val = op.args
                json_ops.append(
                    {
                        "kind": "gen_send",
                        "args": [gen.name, val.name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GEN_THROW":
                gen, val = op.args
                json_ops.append(
                    {
                        "kind": "gen_throw",
                        "args": [gen.name, val.name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GEN_CLOSE":
                json_ops.append(
                    {
                        "kind": "gen_close",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IS_GENERATOR":
                json_ops.append(
                    {
                        "kind": "is_generator",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LOAD_CLOSURE":
                self_ptr, offset = op.args
                json_ops.append(
                    {
                        "kind": "closure_load",
                        "args": [self_ptr],
                        "value": offset,
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STORE_CLOSURE":
                self_ptr, offset, val = op.args
                json_ops.append(
                    {
                        "kind": "closure_store",
                        "args": [self_ptr, val.name],
                        "value": offset,
                    }
                )

        if ops and ops[-1].kind != "ret":
            json_ops.append({"kind": "ret_void"})
        return json_ops

    def to_json(self) -> dict[str, Any]:
        funcs_json: list[dict[str, Any]] = []
        for name, data in self.funcs_map.items():
            funcs_json.append(
                {
                    "name": name,
                    "params": data["params"],
                    "ops": self.map_ops_to_json(data["ops"]),
                }
            )
        return {"functions": funcs_json}


def compile_to_tir(
    source: str,
    parse_codec: Literal["msgpack", "cbor", "json"] = "msgpack",
    type_hint_policy: Literal["ignore", "trust", "check"] = "ignore",
    fallback_policy: FallbackPolicy = "error",
) -> dict[str, Any]:
    tree = ast.parse(source)
    gen = SimpleTIRGenerator(
        parse_codec=parse_codec,
        type_hint_policy=type_hint_policy,
        fallback_policy=fallback_policy,
    )
    gen.visit(tree)
    return gen.to_json()
