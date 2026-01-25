from __future__ import annotations

import ast
from dataclasses import dataclass
from pathlib import Path
import string as _py_string
from typing import TYPE_CHECKING, Any, Callable, Literal, Sequence, TypedDict, cast

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


@dataclass
class ActiveException:
    value: MoltValue
    slot: int | None = None


@dataclass(frozen=True)
class BuiltinFuncSpec:
    runtime: str
    params: tuple[str, ...]
    defaults: tuple[ast.expr, ...] = ()
    vararg: str | None = None
    pos_or_kw_params: tuple[str, ...] = ()
    kwonly_params: tuple[str, ...] = ()
    kw_defaults: tuple[ast.expr | None, ...] = ()


@dataclass(frozen=True)
class FormatLiteral:
    text: str


@dataclass(frozen=True)
class FormatField:
    key: int | str
    rest: list[tuple[bool, int | str]]
    conversion: str | None
    format_spec: list["FormatToken"] | None


FormatToken = FormatLiteral | FormatField


@dataclass
class FormatParseState:
    next_auto: int = 0
    used_auto: bool = False
    used_manual: bool = False


GEN_SEND_OFFSET = 0
GEN_THROW_OFFSET = 8
GEN_CLOSED_OFFSET = 16
GEN_CONTROL_SIZE = 48

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
    "set": 17,
    "frozenset": 18,
    "range": 11,
    "slice": 12,
    "memoryview": 15,
    "object": 100,
    "type": 101,
    "BaseException": 102,
    "Exception": 103,
}

BUILTIN_EXCEPTION_NAMES = {
    "BaseException",
    "BaseExceptionGroup",
    "Exception",
    "ExceptionGroup",
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "BufferError",
    "EOFError",
    "FloatingPointError",
    "GeneratorExit",
    "ImportError",
    "ModuleNotFoundError",
    "IndexError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "MemoryError",
    "NameError",
    "UnboundLocalError",
    "NotImplementedError",
    "OSError",
    "EnvironmentError",
    "IOError",
    "WindowsError",
    "BlockingIOError",
    "ChildProcessError",
    "ConnectionError",
    "BrokenPipeError",
    "ConnectionAbortedError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "FileExistsError",
    "OverflowError",
    "PermissionError",
    "FileNotFoundError",
    "InterruptedError",
    "IsADirectoryError",
    "NotADirectoryError",
    "RecursionError",
    "ReferenceError",
    "RuntimeError",
    "StopIteration",
    "StopAsyncIteration",
    "SyntaxError",
    "IndentationError",
    "TabError",
    "SystemError",
    "SystemExit",
    "TimeoutError",
    "ProcessLookupError",
    "TypeError",
    "UnicodeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "UnicodeTranslateError",
    "ValueError",
    "ZeroDivisionError",
    "Warning",
    "DeprecationWarning",
    "PendingDeprecationWarning",
    "RuntimeWarning",
    "SyntaxWarning",
    "UserWarning",
    "FutureWarning",
    "ImportWarning",
    "UnicodeWarning",
    "BytesWarning",
    "ResourceWarning",
    "EncodingWarning",
}

_MOLT_MISSING = ast.Name(id="__molt_missing__", ctx=ast.Load())
_MOLT_CLOSURE_PARAM = "__molt_closure__"
MOLT_BIND_KIND_OPEN = 1

BUILTIN_FUNC_SPECS: dict[str, BuiltinFuncSpec] = {
    "isinstance": BuiltinFuncSpec("molt_isinstance", ("obj", "classinfo")),
    "issubclass": BuiltinFuncSpec("molt_issubclass", ("sub", "classinfo")),
    "len": BuiltinFuncSpec("molt_len", ("obj",)),
    "hash": BuiltinFuncSpec("molt_hash_builtin", ("obj",)),
    "ord": BuiltinFuncSpec("molt_ord", ("obj",)),
    "chr": BuiltinFuncSpec("molt_chr", ("obj",)),
    "abs": BuiltinFuncSpec("molt_abs_builtin", ("obj",)),
    "ascii": BuiltinFuncSpec("molt_ascii_from_obj", ("obj",)),
    "bin": BuiltinFuncSpec("molt_bin_builtin", ("obj",)),
    "oct": BuiltinFuncSpec("molt_oct_builtin", ("obj",)),
    "hex": BuiltinFuncSpec("molt_hex_builtin", ("obj",)),
    "divmod": BuiltinFuncSpec("molt_divmod_builtin", ("a", "b")),
    "repr": BuiltinFuncSpec("molt_repr_builtin", ("obj",)),
    "format": BuiltinFuncSpec(
        "molt_format_builtin",
        ("value",),
        (ast.Constant(""),),
        pos_or_kw_params=("format_spec",),
    ),
    "callable": BuiltinFuncSpec("molt_callable_builtin", ("obj",)),
    "id": BuiltinFuncSpec("molt_id", ("obj",)),
    "enumerate": BuiltinFuncSpec(
        "molt_enumerate_builtin", ("iterable", "start"), (_MOLT_MISSING,)
    ),
    "round": BuiltinFuncSpec(
        "molt_round_builtin", ("value", "ndigits"), (_MOLT_MISSING,)
    ),
    "iter": BuiltinFuncSpec("molt_iter_checked", ("obj",)),
    "map": BuiltinFuncSpec("molt_map_builtin", ("func",), vararg="iterables"),
    "filter": BuiltinFuncSpec("molt_filter_builtin", ("func", "iterable")),
    "zip": BuiltinFuncSpec("molt_zip_builtin", (), vararg="iterables"),
    "reversed": BuiltinFuncSpec("molt_reversed_builtin", ("seq",)),
    "any": BuiltinFuncSpec("molt_any_builtin", ("iterable",)),
    "all": BuiltinFuncSpec("molt_all_builtin", ("iterable",)),
    "sum": BuiltinFuncSpec(
        "molt_sum_builtin",
        ("iterable",),
        (ast.Constant(0),),
        pos_or_kw_params=("start",),
    ),
    "min": BuiltinFuncSpec(
        "molt_min_builtin",
        (),
        vararg="args",
        kwonly_params=("key", "default"),
        kw_defaults=(ast.Constant(None), _MOLT_MISSING),
    ),
    "max": BuiltinFuncSpec(
        "molt_max_builtin",
        (),
        vararg="args",
        kwonly_params=("key", "default"),
        kw_defaults=(ast.Constant(None), _MOLT_MISSING),
    ),
    "sorted": BuiltinFuncSpec(
        "molt_sorted_builtin",
        ("iterable",),
        kwonly_params=("key", "reverse"),
        kw_defaults=(ast.Constant(None), ast.Constant(False)),
    ),
    "dir": BuiltinFuncSpec("molt_dir_builtin", ("obj",)),
    "open": BuiltinFuncSpec(
        "molt_open_builtin",
        (),
        (
            ast.Constant("r"),
            ast.Constant(-1),
            ast.Constant(None),
            ast.Constant(None),
            ast.Constant(None),
            ast.Constant(True),
            ast.Constant(None),
        ),
        pos_or_kw_params=(
            "file",
            "mode",
            "buffering",
            "encoding",
            "errors",
            "newline",
            "closefd",
            "opener",
        ),
    ),
    "next": BuiltinFuncSpec(
        "molt_next_builtin", ("iterator", "default"), (_MOLT_MISSING,)
    ),
    "aiter": BuiltinFuncSpec("molt_aiter", ("obj",)),
    "anext": BuiltinFuncSpec(
        "molt_anext_builtin", ("aiter", "default"), (_MOLT_MISSING,)
    ),
    "getattr": BuiltinFuncSpec(
        "molt_getattr_builtin", ("obj", "name", "default"), (_MOLT_MISSING,)
    ),
    "setattr": BuiltinFuncSpec("molt_set_attr_name", ("obj", "name", "value")),
    "delattr": BuiltinFuncSpec("molt_del_attr_name", ("obj", "name")),
    "hasattr": BuiltinFuncSpec("molt_has_attr_name", ("obj", "name")),
    "super": BuiltinFuncSpec("molt_super_builtin", ("type", "obj")),
    "print": BuiltinFuncSpec(
        "molt_print_builtin",
        (),
        (),
        vararg="args",
        kwonly_params=("sep", "end", "file", "flush"),
        kw_defaults=(
            ast.Constant(" "),
            ast.Constant("\n"),
            ast.Constant(None),
            ast.Constant(False),
        ),
    ),
    "_molt_getrecursionlimit": BuiltinFuncSpec("molt_getrecursionlimit", ()),
    "_molt_setrecursionlimit": BuiltinFuncSpec("molt_setrecursionlimit", ("limit",)),
    "_molt_getargv": BuiltinFuncSpec("molt_getargv", ()),
    "_molt_exception_last": BuiltinFuncSpec("molt_exception_last", ()),
    "_molt_exception_active": BuiltinFuncSpec("molt_exception_active", ()),
    "_molt_getpid": BuiltinFuncSpec("molt_getpid", ()),
    "_molt_time_monotonic": BuiltinFuncSpec("molt_time_monotonic", ()),
    "_molt_time_monotonic_ns": BuiltinFuncSpec("molt_time_monotonic_ns", ()),
    "_molt_time_time": BuiltinFuncSpec("molt_time_time", ()),
    "_molt_time_time_ns": BuiltinFuncSpec("molt_time_time_ns", ()),
    "_molt_env_get_raw": BuiltinFuncSpec("molt_env_get", ("key", "default")),
    "_molt_errno_constants": BuiltinFuncSpec("molt_errno_constants", ()),
    "_molt_socket_constants": BuiltinFuncSpec("molt_socket_constants", ()),
    "_molt_socket_has_ipv6": BuiltinFuncSpec("molt_socket_has_ipv6", ()),
    "_molt_socket_new": BuiltinFuncSpec(
        "molt_socket_new", ("family", "type", "proto", "fileno")
    ),
    "_molt_socket_close": BuiltinFuncSpec("molt_socket_close", ("sock",)),
    "_molt_socket_drop": BuiltinFuncSpec("molt_socket_drop", ("sock",)),
    "_molt_socket_clone": BuiltinFuncSpec("molt_socket_clone", ("sock",)),
    "_molt_socket_fileno": BuiltinFuncSpec("molt_socket_fileno", ("sock",)),
    "_molt_socket_gettimeout": BuiltinFuncSpec("molt_socket_gettimeout", ("sock",)),
    "_molt_socket_settimeout": BuiltinFuncSpec(
        "molt_socket_settimeout", ("sock", "timeout")
    ),
    "_molt_socket_setblocking": BuiltinFuncSpec(
        "molt_socket_setblocking", ("sock", "flag")
    ),
    "_molt_socket_getblocking": BuiltinFuncSpec("molt_socket_getblocking", ("sock",)),
    "_molt_socket_bind": BuiltinFuncSpec("molt_socket_bind", ("sock", "addr")),
    "_molt_socket_listen": BuiltinFuncSpec("molt_socket_listen", ("sock", "backlog")),
    "_molt_socket_accept": BuiltinFuncSpec("molt_socket_accept", ("sock",)),
    "_molt_socket_connect": BuiltinFuncSpec("molt_socket_connect", ("sock", "addr")),
    "_molt_socket_connect_ex": BuiltinFuncSpec(
        "molt_socket_connect_ex", ("sock", "addr")
    ),
    "_molt_socket_recv": BuiltinFuncSpec("molt_socket_recv", ("sock", "size", "flags")),
    "_molt_socket_recv_into": BuiltinFuncSpec(
        "molt_socket_recv_into", ("sock", "buffer", "size", "flags")
    ),
    "_molt_socket_send": BuiltinFuncSpec("molt_socket_send", ("sock", "data", "flags")),
    "_molt_socket_sendall": BuiltinFuncSpec(
        "molt_socket_sendall", ("sock", "data", "flags")
    ),
    "_molt_socket_sendto": BuiltinFuncSpec(
        "molt_socket_sendto", ("sock", "data", "flags", "addr")
    ),
    "_molt_socket_recvfrom": BuiltinFuncSpec(
        "molt_socket_recvfrom", ("sock", "size", "flags")
    ),
    "_molt_socket_shutdown": BuiltinFuncSpec("molt_socket_shutdown", ("sock", "how")),
    "_molt_socket_getsockname": BuiltinFuncSpec("molt_socket_getsockname", ("sock",)),
    "_molt_socket_getpeername": BuiltinFuncSpec("molt_socket_getpeername", ("sock",)),
    "_molt_socket_setsockopt": BuiltinFuncSpec(
        "molt_socket_setsockopt", ("sock", "level", "optname", "value")
    ),
    "_molt_socket_getsockopt": BuiltinFuncSpec(
        "molt_socket_getsockopt", ("sock", "level", "optname", "buflen")
    ),
    "_molt_socket_detach": BuiltinFuncSpec("molt_socket_detach", ("sock",)),
    "_molt_socketpair": BuiltinFuncSpec("molt_socketpair", ("family", "type", "proto")),
    "_molt_socket_getaddrinfo": BuiltinFuncSpec(
        "molt_socket_getaddrinfo",
        ("host", "port", "family", "type", "proto", "flags"),
    ),
    "_molt_socket_getnameinfo": BuiltinFuncSpec(
        "molt_socket_getnameinfo", ("addr", "flags")
    ),
    "_molt_socket_gethostname": BuiltinFuncSpec("molt_socket_gethostname", ()),
    "_molt_socket_getservbyname": BuiltinFuncSpec(
        "molt_socket_getservbyname", ("name", "proto")
    ),
    "_molt_socket_getservbyport": BuiltinFuncSpec(
        "molt_socket_getservbyport", ("port", "proto")
    ),
    "_molt_socket_inet_pton": BuiltinFuncSpec(
        "molt_socket_inet_pton", ("family", "address")
    ),
    "_molt_socket_inet_ntop": BuiltinFuncSpec(
        "molt_socket_inet_ntop", ("family", "packed")
    ),
    "_molt_io_wait_new": BuiltinFuncSpec(
        "molt_io_wait_new", ("socket", "events", "timeout")
    ),
    "molt_process_spawn": BuiltinFuncSpec(
        "molt_process_spawn", ("args", "env", "cwd", "stdin", "stdout", "stderr")
    ),
    "molt_process_wait_future": BuiltinFuncSpec("molt_process_wait_future", ("proc",)),
    "molt_process_pid": BuiltinFuncSpec("molt_process_pid", ("proc",)),
    "molt_process_returncode": BuiltinFuncSpec("molt_process_returncode", ("proc",)),
    "molt_process_kill": BuiltinFuncSpec("molt_process_kill", ("proc",)),
    "molt_process_terminate": BuiltinFuncSpec("molt_process_terminate", ("proc",)),
    "molt_process_stdin": BuiltinFuncSpec("molt_process_stdin", ("proc",)),
    "molt_process_stdout": BuiltinFuncSpec("molt_process_stdout", ("proc",)),
    "molt_process_stderr": BuiltinFuncSpec("molt_process_stderr", ("proc",)),
    "molt_process_drop": BuiltinFuncSpec("molt_process_drop", ("proc",)),
    "molt_stream_new": BuiltinFuncSpec("molt_stream_new", ("capacity",)),
    "molt_stream_clone": BuiltinFuncSpec("molt_stream_clone", ("stream",)),
    "molt_stream_send_obj": BuiltinFuncSpec("molt_stream_send_obj", ("stream", "data")),
    "molt_stream_recv": BuiltinFuncSpec("molt_stream_recv", ("stream",)),
    "molt_stream_close": BuiltinFuncSpec("molt_stream_close", ("stream",)),
    "molt_stream_drop": BuiltinFuncSpec("molt_stream_drop", ("stream",)),
    "_molt_path_exists": BuiltinFuncSpec("molt_path_exists", ("path",)),
    "_molt_path_unlink": BuiltinFuncSpec("molt_path_unlink", ("path",)),
    "vars": BuiltinFuncSpec("molt_vars_builtin", ("obj",)),
    "_molt_heapq_heapify": BuiltinFuncSpec("molt_heapq_heapify", ("list_obj",)),
    "_molt_heapq_heappush": BuiltinFuncSpec(
        "molt_heapq_heappush", ("list_obj", "item")
    ),
    "_molt_heapq_heappop": BuiltinFuncSpec("molt_heapq_heappop", ("list_obj",)),
    "_molt_heapq_heapreplace": BuiltinFuncSpec(
        "molt_heapq_heapreplace", ("list_obj", "item")
    ),
    "_molt_heapq_heappushpop": BuiltinFuncSpec(
        "molt_heapq_heappushpop", ("list_obj", "item")
    ),
}

MOLT_REEXPORT_FUNCTIONS = {
    "cancel_current": "molt.concurrency",
    "cancelled": "molt.concurrency",
    "CancellationToken": "molt.concurrency",
    "Channel": "molt.concurrency",
    "channel": "molt.concurrency",
    "current_token": "molt.concurrency",
    "set_current_token": "molt.concurrency",
    "spawn": "molt.concurrency",
    "Request": "molt.net",
    "Response": "molt.net",
    "Stream": "molt.net",
    "StreamSender": "molt.net",
    "WebSocket": "molt.net",
    "stream": "molt.net",
    "stream_channel": "molt.net",
    "ws_connect": "molt.net",
    "ws_pair": "molt.net",
}

MOLT_DIRECT_CALLS = {
    "molt": {
        "CancellationToken",
        "Channel",
        "Request",
        "Response",
        "Stream",
        "StreamSender",
        "WebSocket",
        "cancel_current",
        "cancelled",
        "channel",
        "current_token",
        "set_current_token",
        "spawn",
        "stream",
        "stream_channel",
        "ws_connect",
        "ws_pair",
    },
    "molt.concurrency": {
        "CancellationToken",
        "Channel",
        "cancel_current",
        "cancelled",
        "channel",
        "current_token",
        "set_current_token",
        "spawn",
    },
    "molt.net": {
        "Request",
        "Response",
        "Stream",
        "StreamSender",
        "WebSocket",
        "stream",
        "stream_channel",
        "ws_connect",
        "ws_pair",
    },
    "asyncio": {
        "create_task",
        "current_task",
        "ensure_future",
        "gather",
        "get_event_loop",
        "get_running_loop",
        "new_event_loop",
        "run",
        "set_event_loop",
        "sleep",
    },
    "contextlib": {"closing", "nullcontext"},
    "contextvars": {"copy_context"},
    "copy": {"copy", "deepcopy"},
    "dataclasses": {"dataclass", "field"},
    "fnmatch": {"fnmatch", "fnmatchcase"},
    "functools": {"lru_cache", "partial", "reduce", "update_wrapper", "wraps"},
    "importlib": {"import_module", "invalidate_caches", "reload"},
    "inspect": {
        "cleandoc",
        "getdoc",
        "isfunction",
        "isclass",
        "ismodule",
        "iscoroutinefunction",
        "isgeneratorfunction",
        "signature",
    },
    "io": {"open", "stream"},
    "os": {"getenv", "unlink"},
    "pprint": {"pformat", "pprint"},
    "string": {"capwords"},
    "sys": {
        "exc_info",
        "getdefaultencoding",
        "getfilesystemencoding",
        "getrecursionlimit",
        "_getframe",
        "setrecursionlimit",
    },
    "itertools": {"chain", "islice", "repeat"},
    "traceback": {
        "format_exception",
        "format_exception_only",
        "format_exc",
        "format_tb",
        "print_exception",
        "print_exc",
        "print_tb",
    },
    "threading": {"Thread"},
    "typing": {
        "TypeVar",
        "cast",
        "get_args",
        "get_origin",
        "overload",
        "runtime_checkable",
    },
    "warnings": {
        "catch_warnings",
        "filterwarnings",
        "formatwarning",
        "resetwarnings",
        "showwarning",
        "simplefilter",
        "warn",
        "warn_explicit",
    },
}

MOLT_DIRECT_CALL_BIND_ALWAYS = {
    "asyncio": {"gather"},
    "functools": {"partial"},
    "operator": {"attrgetter", "itemgetter", "methodcaller"},
    "itertools": {"chain"},
}

STDLIB_DIRECT_CALL_MODULES = {
    module for module in MOLT_DIRECT_CALLS if not module.startswith("molt.")
}


@dataclass
class TryScope:
    ctx_mark: MoltValue
    finalbody: list[ast.stmt] | None
    ctx_mark_offset: int | None = None


class MethodInfo(TypedDict):
    func: MoltValue
    attr: MoltValue
    descriptor: Literal[
        "function", "classmethod", "staticmethod", "property", "decorated"
    ]
    return_hint: str | None
    param_count: int
    defaults: list[dict[str, Any]]
    has_vararg: bool
    has_varkw: bool
    has_closure: bool
    property_field: str | None


class ClassInfo(TypedDict, total=False):
    fields: dict[str, int]
    field_hints: dict[str, str]
    size: int
    field_order: list[str]
    defaults: dict[str, ast.expr]
    class_attrs: dict[str, ast.expr]
    module: str
    base: str | None
    bases: list[str]
    mro: list[str]
    dynamic: bool
    static: bool
    dataclass: bool
    frozen: bool
    eq: bool
    repr: bool
    slots: bool
    methods: dict[str, MethodInfo]
    layout_version: int
    exception_subclass: bool


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
        known_classes: dict[str, ClassInfo] | None = None,
        stdlib_allowlist: set[str] | None = None,
        known_func_defaults: dict[str, dict[str, dict[str, Any]]] | None = None,
    ) -> None:
        self.funcs_map: dict[str, FuncInfo] = {"molt_main": {"params": [], "ops": []}}
        self.current_func_name: str = "molt_main"
        self.current_ops: list[MoltOp] = self.funcs_map["molt_main"]["ops"]
        self.func_code_ids: dict[str, int] = {}
        self.code_id_counter = 0
        self.code_slots_emitted = False
        self.var_count: int = 0
        self.state_count: int = 0
        self.classes: dict[str, ClassInfo] = dict(known_classes or {})
        self.local_class_names: set[str] = set()
        self.locals: dict[str, MoltValue] = {}
        self.boxed_locals: dict[str, MoltValue] = {}
        self.boxed_local_hints: dict[str, str] = {}
        self.free_vars: dict[str, int] = {}
        self.free_var_hints: dict[str, str] = {}
        self.global_decls: set[str] = set()
        self.nonlocal_decls: set[str] = set()
        self.scope_assigned: set[str] = set()
        self.del_targets: set[str] = set()
        self.unbound_check_names: set[str] = set()
        self.exact_locals: dict[str, str] = {}
        self.globals: dict[str, MoltValue] = {}
        self.func_symbol_names: dict[str, str] = {}
        self.func_default_specs: dict[str, dict[str, Any]] = {}
        self.stable_module_funcs: set[str] = set()
        self.module_declared_funcs: dict[str, str] = {}
        self.module_declared_classes: set[str] = set()
        self.module_defined_funcs: set[str] = set()
        self.module_global_mutations: set[str] = set()
        self.mutated_classes: set[str] = set()
        self.instance_attr_mutations: dict[str, set[str]] = {}
        self.imported_names: dict[str, str] = {}
        self.global_imported_names: dict[str, str] = {}
        self.imported_modules: dict[str, str] = {}
        self.global_imported_modules: dict[str, str] = {}
        self.async_locals: dict[str, int] = {}
        self.async_locals_base: int = 0
        self.async_closure_offset: int | None = None
        self.async_local_hints: dict[str, str] = {}
        self.parse_codec = parse_codec
        self.type_hint_policy = type_hint_policy
        self.explicit_type_hints: dict[str, str] = {}
        self.container_elem_hints: dict[str, str] = {}
        self.global_elem_hints: dict[str, str] = {}
        self.dict_key_hints: dict[str, str] = {}
        self.dict_value_hints: dict[str, str] = {}
        self.stdlib_hint_trust = False
        if source_path:
            normalized_path = source_path.replace("\\", "/")
            if "/src/molt/stdlib/" in normalized_path or normalized_path.startswith(
                "src/molt/stdlib/"
            ):
                self.stdlib_hint_trust = True
        self.global_dict_key_hints: dict[str, str] = {}
        self.global_dict_value_hints: dict[str, str] = {}
        self.type_facts = type_facts
        self.module_name = module_name or "__main__"
        self.entry_module = entry_module
        self.enable_phi = enable_phi
        self.module_prefix = f"{self._sanitize_module_name(self.module_name)}__"
        self.known_modules = set(known_modules or [])
        self.stdlib_allowlist = set(stdlib_allowlist or [])
        self.known_func_defaults: dict[str, dict[str, dict[str, Any]]] = (
            known_func_defaults or {}
        )
        self.module_func_defaults: dict[str, dict[str, Any]] = {}
        self.module_annotations: MoltValue | None = None
        self.module_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.module_annotation_ids: dict[int, int] = {}
        self.module_annotation_exec_map: MoltValue | None = None
        self.module_annotation_exec_name: str | None = None
        self.module_annotation_exec_counter = 0
        self.module_annotation_emitted = False
        self.class_annotation_items: list[tuple[str, ast.expr, int]] = []
        self.class_annotation_exec_map: MoltValue | None = None
        self.class_annotation_exec_name: str | None = None
        self.class_annotation_exec_counter = 0
        self.annotation_name_counter = 0
        self.module_obj: MoltValue | None = None
        self.future_annotations = False
        self.defer_module_attrs = False
        self.deferred_module_attrs: set[str] = set()
        self.fallback_policy = fallback_policy
        self.compat = CompatibilityReporter(fallback_policy, source_path)
        self.source_path = source_path
        self.context_depth = 0
        self.control_flow_depth = 0
        self.try_end_labels: list[int] = []
        self.try_scopes: list[TryScope] = []
        self.try_suppress_depth: int | None = None
        self.return_unwind_depth = 0
        self.return_label: int | None = None
        self.return_slot: MoltValue | None = None
        self.return_slot_index: MoltValue | None = None
        self.return_slot_offset: int | None = None
        self.block_terminated = False
        self.range_loop_stack: list[tuple[MoltValue, MoltValue]] = []
        self.async_index_loop_stack: list[int] = []
        self.loop_break_flags: list[int | str | None] = []
        self.loop_try_depths: list[int] = []
        self.loop_break_counter = 0
        self.loop_layout_guards: list[dict[str, tuple[str, MoltValue]]] = []
        self.loop_guard_assumptions: list[dict[str, tuple[str, bool]]] = []
        self.active_exceptions: list[ActiveException] = []
        self.func_aliases: dict[str, str] = {}
        self.reserved_func_symbols: dict[str, str] = {}
        self.const_ints: dict[str, int] = {}
        self.in_generator = False
        self.async_context = False
        self.lambda_counter = 0
        self.genexpr_counter = 0
        self.qualname_stack: list[tuple[str, bool]] = []
        self.current_class: str | None = None
        self.current_method_first_param: str | None = None
        self.current_line: int | None = None
        self._register_code_symbol("molt_main")
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
            if (
                self.entry_module
                and self.module_name == self.entry_module
                and self.module_name != "__main__"
            ):
                # TODO(import-system, owner:frontend, milestone:TC3, priority:P2, status:partial): split __main__ vs importable module objects when the entry module is imported elsewhere.
                main_name = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__main__"], result=main_name))
                self.emit(
                    MoltOp(
                        kind="MODULE_CACHE_SET",
                        args=[main_name, module_val],
                        result=MoltValue("none"),
                    )
                )
                name_attr = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__name__"], result=name_attr))
                self.emit(
                    MoltOp(
                        kind="MODULE_SET_ATTR",
                        args=[module_val, name_attr, main_name],
                        result=MoltValue("none"),
                    )
                )
            self.module_obj = module_val
        self._emit_module_metadata()
        self._apply_type_facts("molt_main")

    def _emit_module_metadata(self) -> None:
        if self.module_obj is None:
            return
        path_obj: Path | None = None
        if self.source_path:
            path_obj = Path(self.source_path)
            normalized = path_obj.as_posix()
            file_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[normalized], result=file_val))
            self._emit_module_attr_set_on(self.module_obj, "__file__", file_val)
        is_entry_main = self.module_name == "__main__" or (
            self.entry_module
            and self.module_name == self.entry_module
            and self.module_name != "__main__"
        )
        if is_entry_main:
            package_name = ""
        elif path_obj is not None and path_obj.name == "__init__.py":
            package_name = self.module_name
        elif "." in self.module_name:
            package_name = self.module_name.rsplit(".", 1)[0]
        else:
            package_name = ""
        package_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[package_name], result=package_val))
        self._emit_module_attr_set_on(self.module_obj, "__package__", package_val)
        if path_obj is not None and path_obj.name == "__init__.py":
            package_dir = path_obj.parent.as_posix()
            path_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[package_dir], result=path_val))
            list_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[path_val], result=list_val))
            self._emit_module_attr_set_on(self.module_obj, "__path__", list_val)
        # TODO(import-system, owner:frontend, milestone:TC3, priority:P1, status:partial): populate __spec__ once importlib ModuleSpec is available.

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
            mro = [name] + list(bases)
            info["mro"] = mro
            return mro
        mro = [name] + merged
        info["mro"] = mro
        return mro

    def _class_is_exception_subclass(
        self, class_name: str, class_info: ClassInfo
    ) -> bool:
        cached = class_info.get("exception_subclass")
        if cached is not None:
            return cached
        for base_name in self._class_mro_names(class_name)[1:]:
            if base_name in BUILTIN_EXCEPTION_NAMES and base_name not in self.classes:
                class_info["exception_subclass"] = True
                return True
            base_info = self.classes.get(base_name)
            if base_info and self._class_is_exception_subclass(base_name, base_info):
                class_info["exception_subclass"] = True
                return True
        class_info["exception_subclass"] = False
        return False

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
            if isinstance(node, (ast.stmt, ast.ExceptHandler)):
                lineno = getattr(node, "lineno", None)
                if lineno:
                    self._emit_line_marker(int(lineno))
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
            "LOOP_BREAK",
            "LOOP_BREAK_IF_TRUE",
            "LOOP_BREAK_IF_FALSE",
            "LOOP_INDEX_START",
            "LOOP_INDEX_NEXT",
            "STATE_TRANSITION",
            "STATE_YIELD",
            "PHI",
            "EXCEPTION_PUSH",
            "EXCEPTION_POP",
            "EXCEPTION_STACK_CLEAR",
            "EXCEPTION_CLEAR",
            "EXCEPTION_LAST",
            "EXCEPTION_SET_CAUSE",
            "EXCEPTION_SET_LAST",
            "EXCEPTION_CONTEXT_SET",
            "CONTEXT_UNWIND_TO",
            "LINE",
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

    def _emit_line_marker(self, lineno: int) -> None:
        if lineno <= 0:
            return
        if self.current_line == lineno:
            return
        self.current_line = lineno
        self.emit(
            MoltOp(
                kind="LINE",
                args=[lineno],
                result=MoltValue("none"),
            )
        )

    def _fast_int_enabled(self) -> bool:
        return self._hints_enabled()

    def _hints_enabled(self) -> bool:
        return self.type_hint_policy in {"trust", "check"} or self.stdlib_hint_trust

    def _should_fast_int(self, op: MoltOp) -> bool:
        if not self._fast_int_enabled():
            return False
        if op.kind not in {
            "ADD",
            "SUB",
            "MUL",
            "INPLACE_ADD",
            "INPLACE_SUB",
            "INPLACE_MUL",
            "LT",
            "EQ",
            "NE",
        }:
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

    def _emit_open_call(self, node: ast.Call) -> MoltValue:
        mode_expr = None
        if len(node.args) > 1:
            mode_expr = node.args[1]
        for kw in node.keywords:
            if kw.arg == "mode" and mode_expr is None:
                mode_expr = kw.value
        mode_hint = None
        if mode_expr is None:
            mode_hint = "file_text"
        elif isinstance(mode_expr, ast.Constant) and isinstance(mode_expr.value, str):
            mode_hint = "file_bytes" if "b" in mode_expr.value else "file_text"
        res = MoltValue(self.next_var(), type_hint=mode_hint or "file")
        callee = self._emit_builtin_function("open")
        callargs = self._emit_call_args_builder(node)
        self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
        return res

    def _emit_asyncio_sleep(
        self, args: list[ast.expr], keywords: list[ast.keyword]
    ) -> MoltValue:
        delay_expr: ast.expr | None = None
        result_expr: ast.expr | None = None
        if len(args) > 2:
            raise NotImplementedError("asyncio.sleep expects 0-2 arguments")
        if args:
            delay_expr = args[0]
            if len(args) == 2:
                result_expr = args[1]
        for keyword in keywords:
            if keyword.arg is None:
                raise NotImplementedError("asyncio.sleep does not support **kwargs")
            if keyword.arg == "delay":
                if delay_expr is not None:
                    raise NotImplementedError(
                        "asyncio.sleep got multiple values for delay"
                    )
                delay_expr = keyword.value
            elif keyword.arg == "result":
                if result_expr is not None:
                    raise NotImplementedError(
                        "asyncio.sleep got multiple values for result"
                    )
                result_expr = keyword.value
            else:
                raise NotImplementedError(
                    f"asyncio.sleep got unexpected keyword {keyword.arg}"
                )
        if delay_expr is None:
            delay_val = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=delay_val))
        else:
            delay_val = self.visit(delay_expr)
            if delay_val is None:
                raise NotImplementedError("Unsupported delay in asyncio.sleep")
        call_args = [delay_val]
        if result_expr is not None:
            result_val = self.visit(result_expr)
            if result_val is None:
                raise NotImplementedError("Unsupported result in asyncio.sleep")
            call_args.append(result_val)
        res = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(kind="CALL_ASYNC", args=["molt_async_sleep", *call_args], result=res)
        )
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
    def _function_contains_yield(
        node: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> bool:
        def push_arg_annotations(stack: list[ast.AST], args: ast.arguments) -> None:
            for arg in (
                args.posonlyargs
                + args.args
                + args.kwonlyargs
                + ([] if args.vararg is None else [args.vararg])
                + ([] if args.kwarg is None else [args.kwarg])
            ):
                if arg.annotation is not None:
                    stack.append(arg.annotation)

        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, (ast.Yield, ast.YieldFrom)):
                return True
            if isinstance(current, (ast.FunctionDef, ast.AsyncFunctionDef)):
                stack.extend(current.decorator_list)
                stack.extend(current.args.defaults)
                stack.extend(
                    default
                    for default in current.args.kw_defaults
                    if default is not None
                )
                push_arg_annotations(stack, current.args)
                if current.returns is not None:
                    stack.append(current.returns)
                continue
            if isinstance(current, ast.ClassDef):
                stack.extend(current.decorator_list)
                stack.extend(current.bases)
                stack.extend(keyword.value for keyword in current.keywords)
                continue
            if isinstance(current, ast.Lambda):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _async_generator_contains_yield_from(node: ast.AsyncFunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, ast.YieldFrom):
                return True
            if isinstance(
                current,
                (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda),
            ):
                continue
            stack.extend(ast.iter_child_nodes(current))
        return False

    @staticmethod
    def _async_generator_contains_return_value(node: ast.AsyncFunctionDef) -> bool:
        stack: list[ast.AST] = list(node.body)
        while stack:
            current = stack.pop()
            if isinstance(current, ast.Return) and current.value is not None:
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

    def _signature_contains_yield(
        self,
        *,
        decorators: list[ast.expr],
        args: ast.arguments,
        returns: ast.expr | None,
    ) -> bool:
        exprs: list[ast.expr] = list(decorators)
        exprs.extend(args.defaults)
        exprs.extend(expr for expr in args.kw_defaults if expr is not None)
        for arg in (
            args.posonlyargs
            + args.args
            + args.kwonlyargs
            + ([] if args.vararg is None else [args.vararg])
            + ([] if args.kwarg is None else [args.kwarg])
        ):
            if arg.annotation is not None:
                exprs.append(arg.annotation)
        if returns is not None:
            exprs.append(returns)
        return any(self._expr_contains_yield(expr) for expr in exprs)

    def _function_symbol(self, name: str) -> str:
        reserved = self.reserved_func_symbols.pop(name, None)
        if reserved is not None:
            self.func_symbol_names[reserved] = name
            self._register_code_symbol(reserved)
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while symbol in self.funcs_map or f"{symbol}_poll" in self.funcs_map:
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.func_symbol_names[symbol] = name
        self._register_code_symbol(symbol)
        return symbol

    def _reserve_function_symbol(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None:
            return reserved
        base = "molt_user_main" if name == "main" else name
        symbol = f"{self.module_prefix}{base}"
        counter = 1
        while (
            symbol in self.funcs_map
            or f"{symbol}_poll" in self.funcs_map
            or symbol in self.func_symbol_names
            or symbol in self.reserved_func_symbols.values()
        ):
            symbol = f"{self.module_prefix}{base}_{counter}"
            counter += 1
        self.reserved_func_symbols[name] = symbol
        self.func_symbol_names[symbol] = name
        self._register_code_symbol(symbol)
        return symbol

    def _function_symbol_for_reference(self, name: str) -> str:
        reserved = self.reserved_func_symbols.get(name)
        if reserved is not None:
            return reserved
        return self._function_symbol(name)

    def _lambda_symbol(self) -> str:
        self.lambda_counter += 1
        symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        while symbol in self.funcs_map:
            self.lambda_counter += 1
            symbol = f"{self.module_prefix}lambda_{self.lambda_counter}"
        self.func_symbol_names[symbol] = "<lambda>"
        self._register_code_symbol(symbol)
        return symbol

    def _genexpr_symbol(self) -> str:
        self.genexpr_counter += 1
        symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        while symbol in self.funcs_map:
            self.genexpr_counter += 1
            symbol = f"{self.module_prefix}genexpr_{self.genexpr_counter}"
        self.func_symbol_names[symbol] = "<genexpr>"
        self._register_code_symbol(symbol)
        return symbol

    def _register_code_symbol(self, symbol: str) -> int:
        code_id = self.func_code_ids.get(symbol)
        if code_id is None:
            code_id = self.code_id_counter
            self.func_code_ids[symbol] = code_id
            self.code_id_counter += 1
        return code_id

    def _code_symbol_for_value(self, func_val: MoltValue) -> str | None:
        hint = func_val.type_hint
        if isinstance(hint, str):
            if hint.startswith("Func:") or hint.startswith("ClosureFunc:"):
                return hint.split(":", 1)[1]
        return None

    def _qualname_prefix(self) -> str:
        if not self.qualname_stack:
            return ""
        parts: list[str] = []
        for name, is_function in self.qualname_stack:
            parts.append(name)
            if is_function:
                parts.append("<locals>")
        return ".".join(parts)

    def _qualname_for_def(self, name: str) -> str:
        prefix = self._qualname_prefix()
        if not prefix:
            return name
        return f"{prefix}.{name}"

    def _push_qualname(self, name: str, is_function: bool) -> None:
        self.qualname_stack.append((name, is_function))

    def _pop_qualname(self) -> None:
        if self.qualname_stack:
            self.qualname_stack.pop()

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
        self.free_vars = {}
        self.free_var_hints = {}
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        self.del_targets = set()
        self.unbound_check_names = set()
        self.exact_locals = {}
        self.imported_names = dict(self.global_imported_names)
        self.imported_modules = dict(self.global_imported_modules)
        self.async_locals = {}
        self.async_locals_base = 0
        self.async_closure_offset = None
        self.async_local_hints = {}
        self.explicit_type_hints = {}
        self.container_elem_hints = {}
        self.dict_key_hints = {}
        self.dict_value_hints = {}
        self.context_depth = 0
        self.control_flow_depth = 0
        self.const_ints = {}
        self.in_generator = False
        self.async_context = False
        self.current_line = None
        self.try_end_labels = []
        self.try_scopes = []
        self.try_suppress_depth = None
        self.return_unwind_depth = 0
        self.active_exceptions = []
        self.return_label = None
        self.return_slot = None
        self.return_slot_index = None
        self.return_slot_offset = None
        self.block_terminated = False
        if needs_return_slot:
            self._init_return_slot()
        self._apply_type_facts(type_facts_name or name)

    def _module_can_defer_attrs(self, node: ast.Module) -> bool:
        for current in ast.walk(node):
            if isinstance(
                current,
                (
                    ast.FunctionDef,
                    ast.AsyncFunctionDef,
                    ast.ClassDef,
                    ast.Lambda,
                    ast.ListComp,
                    ast.SetComp,
                    ast.DictComp,
                    ast.GeneratorExp,
                ),
            ):
                return False
            if isinstance(current, ast.Call) and isinstance(current.func, ast.Name):
                if current.func.id in {"globals", "locals", "vars"}:
                    return False
        return True

    def _module_has_future_annotations(self, node: ast.Module) -> bool:
        found = False

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                nonlocal found
                if node.module != "__future__":
                    return
                for alias in node.names:
                    if alias.name == "annotations":
                        found = True
                        return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
            if found:
                break
        return found

    def _collect_module_annotation_items(
        self, node: ast.Module
    ) -> tuple[list[tuple[str, ast.expr, int]], dict[int, int]]:
        items: list[tuple[str, ast.expr, int]] = []
        id_map: dict[int, int] = {}

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                if isinstance(node.target, ast.Name):
                    exec_id = len(items)
                    items.append((node.target.id, node.annotation, exec_id))
                    id_map[id(node)] = exec_id

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return items, id_map

    def _collect_global_rebinds(self, node: ast.AST) -> set[str]:
        names: set[str] = set()
        for current in ast.walk(node):
            if isinstance(current, ast.Global):
                names.update(current.names)
        return names

    def _collect_module_assignments(
        self, node: ast.Module
    ) -> tuple[dict[str, int], set[str], bool]:
        counts: dict[str, int] = {}
        func_defs: set[str] = set()
        has_dynamic_bind = False
        outer = self

        def record(name: str) -> None:
            counts[name] = counts.get(name, 0) + 1

        def record_target(target: ast.AST) -> None:
            if isinstance(target, ast.Name):
                record(target.id)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt)
            elif isinstance(target, ast.Starred):
                record_target(target.value)

        def record_pattern(pattern: ast.pattern) -> None:
            if isinstance(pattern, ast.MatchAs):
                if pattern.name:
                    record(pattern.name)
                if pattern.pattern is not None:
                    record_pattern(pattern.pattern)
            elif isinstance(pattern, ast.MatchStar):
                if pattern.name:
                    record(pattern.name)
            elif isinstance(pattern, ast.MatchMapping):
                for sub in pattern.patterns:
                    record_pattern(sub)
                if pattern.rest:
                    record(pattern.rest)
            elif isinstance(pattern, ast.MatchSequence):
                for sub in pattern.patterns:
                    record_pattern(sub)
            elif isinstance(pattern, ast.MatchClass):
                for sub in pattern.patterns:
                    record_pattern(sub)
                for sub in pattern.kwd_patterns:
                    record_pattern(sub)
            elif isinstance(pattern, ast.MatchOr):
                for sub in pattern.patterns:
                    record_pattern(sub)

        class Collector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                return None

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> Any:
                func_defs.add(node.name)
                record(node.name)
                return None

            def visit_ClassDef(self, node: ast.ClassDef) -> Any:
                record(node.name)
                return None

            def visit_Lambda(self, node: ast.Lambda) -> Any:
                return None

            def visit_ListComp(self, node: ast.ListComp) -> Any:
                return None

            def visit_SetComp(self, node: ast.SetComp) -> Any:
                return None

            def visit_DictComp(self, node: ast.DictComp) -> Any:
                return None

            def visit_GeneratorExp(self, node: ast.GeneratorExp) -> Any:
                return None

            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target)
                if node.value is not None:
                    self.visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                record_target(node.target)
                self.visit(node.iter)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                record_target(node.target)
                self.visit(node.iter)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_While(self, node: ast.While) -> None:
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_If(self, node: ast.If) -> None:
                if outer._is_type_checking_test(node.test):
                    for stmt in node.orelse:
                        self.visit(stmt)
                    return None
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    self.visit(item.context_expr)
                    if item.optional_vars is not None:
                        record_target(item.optional_vars)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    self.visit(item.context_expr)
                    if item.optional_vars is not None:
                        record_target(item.optional_vars)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_Try(self, node: ast.Try) -> None:
                for stmt in node.body:
                    self.visit(stmt)
                for handler in node.handlers:
                    self.visit(handler)
                for stmt in node.orelse:
                    self.visit(stmt)
                for stmt in node.finalbody:
                    self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    record(node.name)
                for stmt in node.body:
                    self.visit(stmt)

            def visit_Match(self, node: ast.Match) -> None:
                self.visit(node.subject)
                for case in node.cases:
                    record_pattern(case.pattern)
                    if case.guard is not None:
                        self.visit(case.guard)
                    for stmt in case.body:
                        self.visit(stmt)

            def visit_Import(self, node: ast.Import) -> None:
                for alias in node.names:
                    name = alias.asname or alias.name.split(".", 1)[0]
                    record(name)

            def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
                nonlocal has_dynamic_bind
                for alias in node.names:
                    if alias.name == "*":
                        has_dynamic_bind = True
                        continue
                    name = alias.asname or alias.name
                    record(name)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target)

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return counts, func_defs, has_dynamic_bind

    @classmethod
    def _collect_module_func_defaults(
        cls,
        node: ast.Module,
    ) -> dict[str, dict[str, Any]]:
        defaults: dict[str, dict[str, Any]] = {}
        for stmt in node.body:
            if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            if stmt.args.vararg or stmt.args.kwarg:
                continue
            params = cls._function_param_names(stmt.args)
            default_specs = cls._default_specs_from_args(stmt.args)
            defaults[stmt.name] = {"params": len(params), "defaults": default_specs}
        return defaults

    @staticmethod
    def _default_spec_for_expr(expr: ast.expr) -> dict[str, Any]:
        if isinstance(expr, ast.Constant):
            return {"const": True, "value": expr.value}
        return {"const": False}

    @classmethod
    def _default_specs_from_args(cls, args: ast.arguments) -> list[dict[str, Any]]:
        default_specs = [cls._default_spec_for_expr(expr) for expr in args.defaults]
        if not args.kwonlyargs or not args.kw_defaults:
            return default_specs
        kwonly_names = [arg.arg for arg in args.kwonlyargs]
        kwonly_pairs = list(zip(kwonly_names, args.kw_defaults))
        suffix: list[tuple[str, ast.expr]] = []
        for name, expr in reversed(kwonly_pairs):
            if expr is None:
                break
            suffix.append((name, expr))
        for name, expr in reversed(suffix):
            spec = cls._default_spec_for_expr(expr)
            spec["kwonly"] = True
            spec["name"] = name
            default_specs.append(spec)
        return default_specs

    def _record_func_default_specs(self, func_symbol: str, args: ast.arguments) -> None:
        if args.vararg or args.kwarg:
            return
        params = self._function_param_names(args)
        default_specs = self._default_specs_from_args(args)
        self.func_default_specs[func_symbol] = {
            "params": len(params),
            "defaults": default_specs,
        }

    def _module_stable_funcs(self, node: ast.Module) -> set[str]:
        counts, funcs, dynamic = self._collect_module_assignments(node)
        if dynamic:
            return set()
        global_rebinds = self._collect_global_rebinds(node)
        return {
            name
            for name in funcs
            if counts.get(name, 0) == 1 and name not in global_rebinds
        }

    def _collect_module_func_kinds(self, node: ast.Module) -> dict[str, str]:
        kinds: dict[str, str] = {}
        for stmt in node.body:
            if isinstance(stmt, ast.AsyncFunctionDef):
                kinds[stmt.name] = "async"
            elif isinstance(stmt, ast.FunctionDef):
                if self._function_contains_yield(stmt):
                    kinds[stmt.name] = "gen"
                else:
                    kinds[stmt.name] = "sync"
        return kinds

    def _collect_module_class_names(self, node: ast.Module) -> set[str]:
        return {stmt.name for stmt in node.body if isinstance(stmt, ast.ClassDef)}

    def _collect_module_class_mutations(self, node: ast.Module) -> set[str]:
        class_names = {
            stmt.name for stmt in node.body if isinstance(stmt, ast.ClassDef)
        }
        if not class_names:
            return set()
        mutated: set[str] = set()

        def record_target(target: ast.AST) -> None:
            if isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name):
                if target.value.id in class_names:
                    mutated.add(target.value.id)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt)
            elif isinstance(target, ast.Starred):
                record_target(target.value)

        class Collector(ast.NodeVisitor):
            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target)
                self.visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target)
                if node.value is not None:
                    self.visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target)
                self.visit(node.value)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target)

            def visit_Call(self, node: ast.Call) -> None:
                if (
                    isinstance(node.func, ast.Name)
                    and node.func.id in {"setattr", "delattr"}
                    and node.args
                ):
                    target = node.args[0]
                    if isinstance(target, ast.Name) and target.id in class_names:
                        mutated.add(target.id)
                self.generic_visit(node)

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        return mutated

    def _record_instance_attr_mutation(self, class_name: str, attr: str) -> None:
        if class_name not in self.classes:
            return
        self.instance_attr_mutations.setdefault(class_name, set()).add(attr)

    def _instance_attr_mutated(self, class_name: str, attr: str) -> bool:
        return attr in self.instance_attr_mutations.get(class_name, set())

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
            "free_vars": self.free_vars,
            "free_var_hints": self.free_var_hints,
            "global_decls": self.global_decls,
            "nonlocal_decls": self.nonlocal_decls,
            "scope_assigned": self.scope_assigned,
            "del_targets": self.del_targets,
            "unbound_check_names": self.unbound_check_names,
            "exact_locals": self.exact_locals,
            "async_locals": self.async_locals,
            "async_locals_base": self.async_locals_base,
            "async_closure_offset": self.async_closure_offset,
            "async_local_hints": self.async_local_hints,
            "explicit_type_hints": self.explicit_type_hints,
            "container_elem_hints": self.container_elem_hints,
            "dict_key_hints": self.dict_key_hints,
            "dict_value_hints": self.dict_value_hints,
            "context_depth": self.context_depth,
            "control_flow_depth": self.control_flow_depth,
            "const_ints": self.const_ints,
            "in_generator": self.in_generator,
            "async_context": self.async_context,
            "try_end_labels": self.try_end_labels,
            "try_scopes": self.try_scopes,
            "try_suppress_depth": self.try_suppress_depth,
            "return_unwind_depth": self.return_unwind_depth,
            "active_exceptions": self.active_exceptions,
            "loop_guard_assumptions": self.loop_guard_assumptions,
            "return_label": self.return_label,
            "return_slot": self.return_slot,
            "return_slot_index": self.return_slot_index,
            "return_slot_offset": self.return_slot_offset,
            "defer_module_attrs": self.defer_module_attrs,
            "deferred_module_attrs": self.deferred_module_attrs,
            "imported_names": self.imported_names,
            "imported_modules": self.imported_modules,
        }

    def _restore_function_state(self, state: dict[str, Any]) -> None:
        self.locals = state["locals"]
        self.boxed_locals = state["boxed_locals"]
        self.boxed_local_hints = state["boxed_local_hints"]
        self.free_vars = state["free_vars"]
        self.free_var_hints = state["free_var_hints"]
        self.global_decls = state["global_decls"]
        self.nonlocal_decls = state["nonlocal_decls"]
        self.scope_assigned = state["scope_assigned"]
        self.del_targets = state["del_targets"]
        self.unbound_check_names = state["unbound_check_names"]
        self.exact_locals = state["exact_locals"]
        self.async_locals = state["async_locals"]
        self.async_locals_base = state["async_locals_base"]
        self.async_closure_offset = state["async_closure_offset"]
        self.async_local_hints = state["async_local_hints"]
        self.explicit_type_hints = state["explicit_type_hints"]
        self.container_elem_hints = state["container_elem_hints"]
        self.dict_key_hints = state["dict_key_hints"]
        self.dict_value_hints = state["dict_value_hints"]
        self.context_depth = state["context_depth"]
        self.control_flow_depth = state["control_flow_depth"]
        self.const_ints = state["const_ints"]
        self.in_generator = state["in_generator"]
        self.async_context = state["async_context"]
        self.try_end_labels = state["try_end_labels"]
        self.try_scopes = state["try_scopes"]
        self.try_suppress_depth = state["try_suppress_depth"]
        self.return_unwind_depth = state["return_unwind_depth"]
        self.active_exceptions = state["active_exceptions"]
        self.loop_guard_assumptions = state["loop_guard_assumptions"]
        self.return_label = state["return_label"]
        self.return_slot = state["return_slot"]
        self.return_slot_index = state["return_slot_index"]
        self.return_slot_offset = state["return_slot_offset"]
        self.defer_module_attrs = state["defer_module_attrs"]
        self.deferred_module_attrs = state["deferred_module_attrs"]
        self.imported_names = state["imported_names"]
        self.imported_modules = state["imported_modules"]

    def visit_Module(self, node: ast.Module) -> None:
        defer = self._module_can_defer_attrs(node)
        prev_defer = self.defer_module_attrs
        prev_dirty = self.deferred_module_attrs
        prev_stable = self.stable_module_funcs
        prev_mutated = self.mutated_classes
        prev_declared = self.module_declared_funcs
        prev_declared_classes = self.module_declared_classes
        prev_reserved = self.reserved_func_symbols
        prev_defined = self.module_defined_funcs
        prev_defaults = self.module_func_defaults
        prev_future = self.future_annotations
        prev_annotations = self.module_annotations
        prev_annotation_items = self.module_annotation_items
        prev_annotation_ids = self.module_annotation_ids
        prev_annotation_exec_map = self.module_annotation_exec_map
        prev_annotation_exec_name = self.module_annotation_exec_name
        prev_annotation_exec_counter = self.module_annotation_exec_counter
        prev_annotation_emitted = self.module_annotation_emitted
        prev_global_mutations = self.module_global_mutations
        self.stable_module_funcs = self._module_stable_funcs(node)
        self.mutated_classes = self._collect_module_class_mutations(node)
        self.module_declared_funcs = self._collect_module_func_kinds(node)
        self.module_declared_classes = self._collect_module_class_names(node)
        self.reserved_func_symbols = {}
        for func_name, kind in self.module_declared_funcs.items():
            if kind in {"sync", "async", "gen"}:
                self._reserve_function_symbol(func_name)
        self.module_defined_funcs = set()
        self.module_func_defaults = self.known_func_defaults.get(
            self.module_name, self._collect_module_func_defaults(node)
        )
        self.future_annotations = self._module_has_future_annotations(node)
        self.module_annotations = None
        self.module_annotation_items = []
        self.module_annotation_ids = {}
        self.module_annotation_exec_map = None
        self.module_annotation_exec_name = None
        self.module_annotation_exec_counter = 0
        self.module_annotation_emitted = False
        self.module_global_mutations = set()
        if not self.future_annotations:
            items, id_map = self._collect_module_annotation_items(node)
            if items:
                self.module_annotation_items = items
                self.module_annotation_ids = id_map
                self.module_annotation_exec_counter = len(items)
                self._ensure_module_annotation_exec_map()
                annotate_val = self._emit_annotate_function_obj(
                    items=list(self.module_annotation_items),
                    exec_map_name=self.module_annotation_exec_name,
                    stringize=False,
                )
                self.globals["__annotate__"] = annotate_val
                self.locals["__annotate__"] = annotate_val
                self._emit_module_attr_set("__annotate__", annotate_val)
                self.module_annotation_emitted = True
        if defer:
            self.defer_module_attrs = True
            self.deferred_module_attrs = set()
        for stmt in node.body:
            self.visit(stmt)
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                self.module_defined_funcs.add(stmt.name)
        if defer:
            self._flush_deferred_module_attrs()
        if (
            not self.future_annotations
            and self.module_annotation_items
            and not self.module_annotation_emitted
        ):
            annotate_val = self._emit_annotate_function_obj(
                items=list(self.module_annotation_items),
                exec_map_name=self.module_annotation_exec_name,
                stringize=False,
            )
            self.globals["__annotate__"] = annotate_val
            self.locals["__annotate__"] = annotate_val
            self._emit_module_attr_set("__annotate__", annotate_val)
        if self.current_func_name == "molt_main":
            self._emit_raise_if_pending(emit_exit=True, clear_handlers=True)
            complete_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=complete_val))
            self._emit_module_attr_set("__molt_module_complete__", complete_val)
            self.globals["__molt_module_complete__"] = complete_val
            self.locals["__molt_module_complete__"] = complete_val
        self.defer_module_attrs = prev_defer
        self.deferred_module_attrs = prev_dirty
        self.stable_module_funcs = prev_stable
        self.mutated_classes = prev_mutated
        self.module_declared_funcs = prev_declared
        self.module_declared_classes = prev_declared_classes
        self.reserved_func_symbols = prev_reserved
        self.module_defined_funcs = prev_defined
        self.module_func_defaults = prev_defaults
        self.future_annotations = prev_future
        self.module_annotations = prev_annotations
        self.module_annotation_items = prev_annotation_items
        self.module_annotation_ids = prev_annotation_ids
        self.module_annotation_exec_map = prev_annotation_exec_map
        self.module_annotation_exec_name = prev_annotation_exec_name
        self.module_annotation_exec_counter = prev_annotation_exec_counter
        self.module_annotation_emitted = prev_annotation_emitted
        self.module_global_mutations = prev_global_mutations
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

    def is_async_context(self) -> bool:
        return self.async_context

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

    def _property_field_from_method(self, node: ast.FunctionDef) -> str | None:
        if len(node.body) != 1:
            return None
        stmt = node.body[0]
        if not isinstance(stmt, ast.Return):
            return None
        value = stmt.value
        if not isinstance(value, ast.Attribute):
            return None
        if not isinstance(value.value, ast.Name):
            return None
        if value.value.id != "self":
            return None
        return value.attr

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

    def _class_layout_version(
        self,
        class_name: str,
        class_attrs: dict[str, ast.expr],
        methods: dict[str, MethodInfo] | None = None,
        method_count: int | None = None,
    ) -> int:
        class_info = self.classes[class_name]
        field_offsets = (
            1
            if class_info.get("fields")
            and not class_info.get("dynamic")
            and not class_info.get("dataclass")
            else 0
        )
        if method_count is None:
            method_count = len(methods or {})
        return 1 + field_offsets + len(class_attrs) + method_count

    def _class_layout_stable(self, class_name: str) -> bool:
        class_info = self.classes.get(class_name)
        if not class_info:
            return False
        if class_info.get("dynamic") or class_info.get("dataclass"):
            return False
        if class_name in self.mutated_classes:
            return False
        return True

    def _async_local_offset(self, name: str) -> int:
        if name not in self.async_locals:
            self.async_locals[name] = (
                self.async_locals_base + len(self.async_locals) * 8
            )
        return self.async_locals[name]

    def _init_scope_async_locals(self, arg_nodes: list[ast.arg]) -> None:
        if not self.scope_assigned:
            return
        arg_names = {arg.arg for arg in arg_nodes}
        missing_val: MoltValue | None = None
        for name in sorted(self.scope_assigned):
            if (
                name in arg_names
                or name in self.global_decls
                or name in self.nonlocal_decls
            ):
                continue
            if name in self.async_locals:
                continue
            if missing_val is None:
                missing_val = self._emit_missing_value()
            offset = self._async_local_offset(name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, missing_val],
                    result=MoltValue("none"),
                )
            )

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
        stripped = text.strip()
        if stripped[:1] in {"'", '"'} and stripped[-1:] == stripped[:1]:
            stripped = stripped[1:-1]
        return normalize_type_hint(stripped)

    def _annotation_source(self, node: ast.expr) -> str:
        try:
            return ast.unparse(node)
        except Exception as exc:
            raise NotImplementedError("Unsupported annotation expression") from exc

    def _emit_annotation_value(
        self, node: ast.expr, *, stringize: bool | None = None
    ) -> MoltValue:
        use_string = self.future_annotations if stringize is None else stringize
        if use_string:
            text = self._annotation_source(node)
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[text], result=res))
            return res
        val = self.visit(node)
        if val is None:
            raise NotImplementedError("Unsupported annotation expression")
        return val

    def _annotation_exec_name(self, owner: str) -> str:
        name = f"__molt_annotations_exec_{owner}_{self.annotation_name_counter}"
        self.annotation_name_counter += 1
        return name

    def _annotation_exec_id(self, *, is_module: bool) -> int:
        if is_module:
            ident = self.module_annotation_exec_counter
            self.module_annotation_exec_counter += 1
            return ident
        ident = self.class_annotation_exec_counter
        self.class_annotation_exec_counter += 1
        return ident

    def _collect_annotation_free_vars(self, node: ast.AST) -> list[str]:
        if self.current_func_name == "molt_main":
            return []
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> None:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node)
        used -= self.global_decls
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in used if name in outer_scope)

    def _annotate_qualname(self) -> str:
        prefix = self._qualname_prefix()
        if not prefix:
            return "__annotate__"
        return f"{prefix}.__annotate__"

    def _ensure_module_annotation_exec_map(self) -> MoltValue:
        if self.module_annotation_exec_map is not None:
            return self.module_annotation_exec_map
        owner = self._sanitize_module_name(self.module_name)
        name = self._annotation_exec_name(owner)
        self.module_annotation_exec_name = name
        exec_map = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=exec_map))
        self.module_annotation_exec_map = exec_map
        self._store_local_value(name, exec_map)
        if self.current_func_name == "molt_main":
            self.globals[name] = exec_map
            self._emit_module_attr_set(name, exec_map)
        return exec_map

    def _ensure_class_annotation_exec_map(self, class_name: str) -> MoltValue:
        if self.class_annotation_exec_map is not None:
            return self.class_annotation_exec_map
        owner = self._sanitize_module_name(class_name)
        name = self._annotation_exec_name(owner)
        self.class_annotation_exec_name = name
        exec_map = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=exec_map))
        self.class_annotation_exec_map = exec_map
        self._store_local_value(name, exec_map)
        if self.current_func_name == "molt_main":
            self.globals[name] = exec_map
            self._emit_module_attr_set(name, exec_map)
        return exec_map

    def _emit_annotation_exec_mark(self, exec_map: MoltValue, exec_id: int) -> None:
        key_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[exec_id], result=key_val))
        val_val = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=val_val))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[exec_map, key_val, val_val],
                result=MoltValue("none"),
            )
        )

    def _rewrite_class_annotation_expr(
        self, expr: ast.expr, class_name: str, class_scope: set[str]
    ) -> ast.expr:
        class_name_node = ast.Name(id=class_name, ctx=ast.Load())

        class Rewriter(ast.NodeTransformer):
            def visit_Name(self, node: ast.Name) -> ast.AST:
                if isinstance(node.ctx, ast.Load) and node.id in class_scope:
                    return ast.copy_location(
                        ast.Attribute(
                            value=class_name_node,
                            attr=node.id,
                            ctx=ast.Load(),
                        ),
                        node,
                    )
                return node

            def visit_Lambda(self, node: ast.Lambda) -> ast.AST:
                return node

            def visit_FunctionDef(self, node: ast.FunctionDef) -> ast.AST:
                return node

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> ast.AST:
                return node

            def visit_ClassDef(self, node: ast.ClassDef) -> ast.AST:
                return node

        return cast(ast.expr, Rewriter().visit(expr))

    def _emit_module_annotations_dict(self) -> MoltValue:
        if self.module_annotations is not None:
            return self.module_annotations
        existing = self.locals.get("__annotations__")
        if existing is not None and existing.type_hint == "dict":
            self.module_annotations = existing
            return existing
        ann = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=ann))
        self._emit_module_attr_set("__annotations__", ann)
        if self.current_func_name == "molt_main":
            self.globals["__annotations__"] = ann
        self.locals["__annotations__"] = ann
        self.module_annotations = ann
        return ann

    def _annotation_items_for_function(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> list[tuple[str, ast.expr]]:
        items: list[tuple[str, ast.expr]] = []
        for arg in node.args.posonlyargs + node.args.args:
            if arg.annotation is not None:
                items.append((arg.arg, arg.annotation))
        if node.args.vararg is not None and node.args.vararg.annotation is not None:
            items.append((node.args.vararg.arg, node.args.vararg.annotation))
        for arg in node.args.kwonlyargs:
            if arg.annotation is not None:
                items.append((arg.arg, arg.annotation))
        if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
            items.append((node.args.kwarg.arg, node.args.kwarg.annotation))
        if node.returns is not None:
            items.append(("return", node.returns))
        return items

    def _emit_annotate_function_obj(
        self,
        *,
        items: list[tuple[str, ast.expr, int]],
        exec_map_name: str | None,
        stringize: bool,
        module_override: str | None = None,
    ) -> MoltValue:
        func_symbol = self._function_symbol("__annotate__")
        free_vars: set[str] = set()
        for _name, expr, _exec_id in items:
            free_vars.update(self._collect_annotation_free_vars(expr))
        if exec_map_name and self.current_func_name != "molt_main":
            free_vars.add(exec_map_name)
        free_vars_list = sorted(free_vars)
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if free_vars_list and self.current_func_name != "molt_main":
            self.unbound_check_names.update(free_vars_list)
            for name in free_vars_list:
                self._box_local(name)
            for name in free_vars_list:
                hint = self.boxed_local_hints.get(name)
                if hint is None:
                    value = self.locals.get(name)
                    if value is not None and value.type_hint:
                        hint = value.type_hint
                free_var_hints[name] = hint or "Any"
            closure_items = [self.boxed_locals[name] for name in free_vars_list]
            closure_val = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val))
            has_closure = True
        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, 1, closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(MoltOp(kind="FUNC_NEW", args=[func_symbol, 1], result=func_val))
        self._emit_function_metadata(
            func_val,
            name="__annotate__",
            qualname=self._annotate_qualname(),
            trace_lineno=None,
            posonly_params=["format"],
            pos_or_kw_params=[],
            kwonly_params=[],
            vararg=None,
            varkw=None,
            default_exprs=[],
            kw_default_exprs=[],
            docstring=None,
            module_override=module_override,
        )

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        params = ["format"]
        if has_closure:
            params = [_MOLT_CLOSURE_PARAM] + params
        self.start_function(func_symbol, params=params, type_facts_name="__annotate__")
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars_list)}
            self.free_var_hints = free_var_hints
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = set()
        self.nonlocal_decls = set()
        self.scope_assigned = set()
        self.del_targets = set()
        self.unbound_check_names = set()
        format_val = MoltValue("format", type_hint="int")
        self.locals["format"] = format_val

        one_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one_val))
        is_one = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[format_val, one_val], result=is_one))
        two_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[2], result=two_val))
        is_two = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="EQ", args=[format_val, two_val], result=is_two))
        exec_map_val: MoltValue | None = None
        if exec_map_name is not None:
            exec_map_val = self.visit(ast.Name(id=exec_map_name, ctx=ast.Load()))
        missing_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="MISSING", args=[], result=missing_val))

        def emit_annotation_body(use_stringize: bool) -> None:
            res_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=[], result=res_dict))
            for name, expr, exec_id in items:
                if exec_map_val is not None:
                    key_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[exec_id], result=key_val))
                    exec_flag = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="DICT_GET",
                            args=[exec_map_val, key_val, missing_val],
                            result=exec_flag,
                        )
                    )
                    is_missing = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(
                            kind="IS",
                            args=[exec_flag, missing_val],
                            result=is_missing,
                        )
                    )
                    self.emit(
                        MoltOp(kind="IF", args=[is_missing], result=MoltValue("none"))
                    )
                    self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                value_val = self._emit_annotation_value(expr, stringize=use_stringize)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[res_dict, key_val, value_val],
                        result=MoltValue("none"),
                    )
                )
                if exec_map_val is not None:
                    self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="ret", args=[res_dict], result=MoltValue("none")))

        self.emit(MoltOp(kind="IF", args=[is_one], result=MoltValue("none")))
        emit_annotation_body(stringize)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="IF", args=[is_two], result=MoltValue("none")))
        emit_annotation_body(stringize)
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        msg_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[""], result=msg_val))
        err_val = self._emit_exception_new("NotImplementedError", msg_val)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        return func_val

    def _emit_function_annotate(
        self, func_val: MoltValue, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> None:
        items = self._annotation_items_for_function(node)
        if not items:
            return
        annotated_items = [(name, expr, idx) for idx, (name, expr) in enumerate(items)]
        annotate_val = self._emit_annotate_function_obj(
            items=annotated_items,
            exec_map_name=None,
            stringize=self.future_annotations,
        )
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[func_val, "__annotate__", annotate_val],
                result=MoltValue("none"),
            )
        )

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

    def _emit_module_global_del(self, name: str) -> None:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        module_val = self.module_obj
        if self.current_func_name != "molt_main" or module_val is None:
            module_name = MoltValue(self.next_var(), type_hint="str")
            self.emit(
                MoltOp(kind="CONST_STR", args=[self.module_name], result=module_name)
            )
            module_val = MoltValue(self.next_var(), type_hint="module")
            self.emit(
                MoltOp(kind="MODULE_CACHE_GET", args=[module_name], result=module_val)
            )
        self.emit(
            MoltOp(
                kind="MODULE_DEL_GLOBAL",
                args=[module_val, name_val],
                result=MoltValue("none"),
            )
        )

    def _emit_function_metadata(
        self,
        func_val: MoltValue,
        *,
        name: str,
        qualname: str,
        trace_filename: str | None = None,
        trace_lineno: int | None = None,
        trace_name: str | None = None,
        posonly_params: list[str],
        pos_or_kw_params: list[str],
        kwonly_params: list[str],
        vararg: str | None,
        varkw: str | None,
        default_exprs: list[ast.expr],
        kw_default_exprs: list[ast.expr | None],
        docstring: str | None,
        module_override: str | None = None,
        is_coroutine: bool = False,
        is_generator: bool = False,
        is_async_generator: bool = False,
        bind_kind: int | None = None,
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

        module_name = module_override or self.module_name
        module_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_val))
        set_attr("__module__", module_val)

        arg_name_vals: list[MoltValue] = []
        for param in posonly_params + pos_or_kw_params:
            param_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[param], result=param_val))
            arg_name_vals.append(param_val)
        arg_names_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=arg_name_vals, result=arg_names_tuple))
        set_attr("__molt_arg_names__", arg_names_tuple)

        posonly_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[len(posonly_params)], result=posonly_val))
        set_attr("__molt_posonly__", posonly_val)

        kwonly_name_vals: list[MoltValue] = []
        for param in kwonly_params:
            param_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[param], result=param_val))
            kwonly_name_vals.append(param_val)
        kwonly_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=kwonly_name_vals, result=kwonly_tuple))
        set_attr("__molt_kwonly_names__", kwonly_tuple)

        if vararg is None:
            vararg_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=vararg_val))
        else:
            vararg_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[vararg], result=vararg_val))
        set_attr("__molt_vararg__", vararg_val)

        if varkw is None:
            varkw_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=varkw_val))
        else:
            varkw_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[varkw], result=varkw_val))
        set_attr("__molt_varkw__", varkw_val)

        yield_in_defaults = False
        yield_in_kwdefaults = False
        func_spill: int | None = None
        if self.in_generator:
            yield_in_defaults = any(
                self._expr_contains_yield(expr) for expr in default_exprs
            )
            yield_in_kwdefaults = any(
                self._expr_contains_yield(expr)
                for expr in kw_default_exprs
                if expr is not None
            )
            if yield_in_defaults or yield_in_kwdefaults:
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )

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
            if func_spill is not None and yield_in_defaults:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            set_attr("__defaults__", defaults_tuple)
        else:
            defaults_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=defaults_none))
            set_attr("__defaults__", defaults_none)

        if kw_default_exprs and kwonly_params:
            kw_pairs: list[MoltValue] = []
            for name, expr in zip(kwonly_params, kw_default_exprs):
                if expr is None:
                    continue
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                val = self.visit(expr)
                if val is None:
                    val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                kw_pairs.extend([key_val, val])
            if kw_pairs:
                kw_defaults = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_NEW", args=kw_pairs, result=kw_defaults))
                if func_spill is not None and yield_in_kwdefaults:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                set_attr("__kwdefaults__", kw_defaults)
            else:
                kw_defaults_none = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=kw_defaults_none))
                if func_spill is not None and yield_in_kwdefaults:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                set_attr("__kwdefaults__", kw_defaults_none)
        else:
            kw_defaults_none = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=kw_defaults_none))
            set_attr("__kwdefaults__", kw_defaults_none)

        if bind_kind is not None:
            bind_kind_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[bind_kind], result=bind_kind_val))
            set_attr("__molt_bind_kind__", bind_kind_val)

        if docstring is None:
            doc_val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=doc_val))
        else:
            doc_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[docstring], result=doc_val))
        set_attr("__doc__", doc_val)

        filename = trace_filename or self.source_path or "<unknown>"
        trace_label = trace_name or qualname or name
        file_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[filename], result=file_val))
        line_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="CONST",
                args=[int(trace_lineno or 0)],
                result=line_val,
            )
        )
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[trace_label], result=name_val))
        linetable_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=linetable_val))
        code_val = MoltValue(self.next_var(), type_hint="code")
        self.emit(
            MoltOp(
                kind="CODE_NEW",
                args=[file_val, name_val, line_val, linetable_val],
                result=code_val,
            )
        )
        set_attr("__code__", code_val)
        code_symbol = self._code_symbol_for_value(func_val)
        if code_symbol is not None:
            code_id = self._register_code_symbol(code_symbol)
            self.emit(
                MoltOp(
                    kind="CODE_SLOT_SET",
                    args=[code_val],
                    result=MoltValue("none"),
                    metadata={"code_id": code_id},
                )
            )

        if is_coroutine:
            coro_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=coro_val))
            set_attr("__molt_is_coroutine__", coro_val)
        if is_generator:
            gen_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gen_val))
            set_attr("__molt_is_generator__", gen_val)
        if is_async_generator:
            gen_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=gen_val))
            set_attr("__molt_is_async_generator__", gen_val)

    @staticmethod
    def _split_function_args(
        args: ast.arguments,
    ) -> tuple[list[ast.arg], list[ast.arg], list[ast.arg], str | None, str | None]:
        posonly = list(args.posonlyargs)
        pos_or_kw = list(args.args)
        kwonly = list(args.kwonlyargs)
        vararg = args.vararg.arg if args.vararg else None
        varkw = args.kwarg.arg if args.kwarg else None
        return posonly, pos_or_kw, kwonly, vararg, varkw

    @classmethod
    def _function_param_names(cls, args: ast.arguments) -> list[str]:
        posonly, pos_or_kw, kwonly, vararg, varkw = cls._split_function_args(args)
        names = [arg.arg for arg in posonly + pos_or_kw]
        if vararg is not None:
            names.append(vararg)
        names.extend(arg.arg for arg in kwonly)
        if varkw is not None:
            names.append(varkw)
        return names

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

    def _emit_class_ref(self, class_name: str) -> MoltValue:
        class_info = self.classes.get(class_name)
        module_name = class_info.get("module") if class_info else None
        if module_name and module_name != self.module_name:
            return self._emit_module_attr_get_on(module_name, class_name)
        return self._emit_module_attr_get(class_name)

    def _emit_global_get(self, name: str) -> MoltValue:
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
            MoltOp(kind="MODULE_GET_GLOBAL", args=[module_val, name_val], result=res)
        )
        return res

    def _emit_globals_dict(self) -> MoltValue:
        # TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): expose globals() as a first-class builtin object (not just direct calls).
        module_name = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[self.module_name], result=module_name))
        module_val = MoltValue(self.next_var(), type_hint="module")
        self.emit(
            MoltOp(kind="MODULE_CACHE_GET", args=[module_name], result=module_val)
        )
        dict_name = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__dict__"], result=dict_name))
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, dict_name], result=res)
        )
        return res

    def _emit_locals_dict(self) -> MoltValue:
        if self.current_func_name == "molt_main":
            return self._emit_globals_dict()
        # TODO(introspection, owner:frontend, milestone:TC2, priority:P2, status:partial): keep locals() mappings live-updated on mutation and deletion.
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
        for name in sorted(self.locals):
            if name == _MOLT_CLOSURE_PARAM or name.startswith("__molt_"):
                continue
            value = self._load_local_value_unchecked(name)
            if value is None:
                continue
            missing = self._emit_missing_value()
            is_missing = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="IS", args=[value, missing], result=is_missing))
            self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            key = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[name], result=key))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[res, key, value],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    @staticmethod
    def _normalize_allowlist_module(module_name: str | None) -> str | None:
        if not module_name or module_name == "molt.stdlib":
            return None
        if module_name.startswith("molt.stdlib."):
            return module_name[len("molt.stdlib.") :]
        return module_name

    @staticmethod
    def _display_allowlist_module(module_name: str) -> str:
        if module_name in STDLIB_DIRECT_CALL_MODULES:
            return f"molt.stdlib.{module_name}"
        return module_name

    def _call_allowlist_suggestion(
        self, func_id: str, imported_from: str | None
    ) -> str | None:
        if imported_from == "molt":
            target_module = MOLT_REEXPORT_FUNCTIONS.get(func_id)
            if target_module:
                return f"{target_module}.{func_id}"
        if imported_from:
            normalized = self._normalize_allowlist_module(imported_from)
            if (
                normalized
                and normalized in MOLT_DIRECT_CALLS
                and func_id in MOLT_DIRECT_CALLS[normalized]
            ):
                display_module = self._display_allowlist_module(normalized)
                return f"{display_module}.{func_id}"
            if (
                imported_from in MOLT_DIRECT_CALLS
                and func_id in MOLT_DIRECT_CALLS[imported_from]
            ):
                display_module = self._display_allowlist_module(imported_from)
                return f"{display_module}.{func_id}"
        return None

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

    def _lookup_func_defaults(
        self, module_name: str | None, func_id: str
    ) -> dict[str, Any] | None:
        if module_name is None:
            module_name = self.module_name
        normalized = self._normalize_allowlist_module(module_name)
        if normalized is not None:
            module_name = normalized
        module_defaults = self.known_func_defaults.get(module_name)
        if module_defaults is None and module_name == self.module_name:
            module_defaults = self.module_func_defaults
        if module_defaults is None:
            return None
        return module_defaults.get(func_id)

    def _emit_module_attr_get_on(self, module_name: str, name: str) -> MoltValue:
        module_val = self._emit_module_load(module_name)
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=name_val))
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="MODULE_GET_ATTR", args=[module_val, name_val], result=res)
        )
        return res

    def _emit_function_defaults_tuple(self, func_obj: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[func_obj, "__defaults__"],
                result=res,
            )
        )
        return res

    def _emit_function_kwdefaults_dict(self, func_obj: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[func_obj, "__kwdefaults__"],
                result=res,
            )
        )
        return res

    def _emit_bound_method_func(self, method_obj: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[method_obj, "__func__"],
                result=res,
            )
        )
        return res

    def _emit_class_method_func(
        self, class_obj: MoltValue, method_name: str
    ) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_OBJ",
                args=[class_obj, method_name],
                result=res,
            )
        )
        return res

    def _apply_default_specs(
        self,
        total_params: int | None,
        default_specs: list[dict[str, Any]],
        args: list[MoltValue],
        node: ast.AST,
        *,
        call_name: str,
        func_obj: MoltValue | None = None,
        implicit_self: bool = False,
    ) -> list[MoltValue]:
        if total_params is None:
            return args
        arg_count = len(args) + (1 if implicit_self else 0)
        if arg_count > total_params:
            raise NotImplementedError(
                f"{call_name} expects at most {total_params} arguments"
            )
        missing = total_params - arg_count
        if missing <= 0:
            return args
        if missing > len(default_specs):
            raise NotImplementedError(
                f"{call_name} expects at least {total_params - len(default_specs)}"
                " arguments"
            )
        base_index = len(default_specs) - missing
        specs_slice = default_specs[base_index : base_index + missing]
        needs_tuple = any(
            not spec.get("const", False) and not spec.get("kwonly", False)
            for spec in specs_slice
        )
        needs_kwdefaults = any(
            not spec.get("const", False) and spec.get("kwonly", False)
            for spec in specs_slice
        )
        defaults_tuple: MoltValue | None = None
        kwdefaults_dict: MoltValue | None = None
        if needs_tuple or needs_kwdefaults:
            if func_obj is None:
                raise self.compat.unsupported(
                    node,
                    f"call to {call_name} with non-constant defaults",
                    impact="medium",
                    alternative="pass explicit arguments",
                    detail="only literal defaults are supported for direct calls",
                )
            if needs_tuple:
                defaults_tuple = self._emit_function_defaults_tuple(func_obj)
            if needs_kwdefaults:
                kwdefaults_dict = self._emit_function_kwdefaults_dict(func_obj)
        missing_val: MoltValue | None = None
        for offset, spec in enumerate(specs_slice):
            if spec.get("const", False):
                args.append(self._emit_const_value(spec.get("value")))
                continue
            if spec.get("kwonly", False):
                if kwdefaults_dict is None:
                    raise self.compat.unsupported(
                        node,
                        f"call to {call_name} with non-constant defaults",
                        impact="medium",
                        alternative="pass explicit arguments",
                        detail="only literal defaults are supported for direct calls",
                    )
                if missing_val is None:
                    missing_val = self._emit_missing_value()
                key_name = spec.get("name")
                if not isinstance(key_name, str):
                    raise NotImplementedError("Invalid kwonly default spec name")
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[key_name], result=key_val))
                default_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="DICT_GET",
                        args=[kwdefaults_dict, key_val, missing_val],
                        result=default_val,
                    )
                )
                args.append(default_val)
                continue
            if defaults_tuple is None:
                raise self.compat.unsupported(
                    node,
                    f"call to {call_name} with non-constant defaults",
                    impact="medium",
                    alternative="pass explicit arguments",
                    detail="only literal defaults are supported for direct calls",
                )
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[base_index + offset], result=idx_val))
            default_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="INDEX",
                    args=[defaults_tuple, idx_val],
                    result=default_val,
                )
            )
            args.append(default_val)
        return args

    def _emit_const_value(self, value: object) -> MoltValue:
        if value is None:
            res = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            return res
        if isinstance(value, bool):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[value], result=res))
            return res
        if isinstance(value, int):
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[value], result=res))
            return res
        if isinstance(value, float):
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[value], result=res))
            return res
        if isinstance(value, str):
            res = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[value], result=res))
            return res
        if isinstance(value, bytes):
            res = MoltValue(self.next_var(), type_hint="bytes")
            self.emit(MoltOp(kind="CONST_BYTES", args=[value], result=res))
            return res
        raise NotImplementedError(f"Unsupported default literal: {value!r}")

    def _apply_direct_call_defaults(
        self,
        module_name: str | None,
        func_id: str,
        args: list[MoltValue],
        node: ast.AST,
    ) -> list[MoltValue]:
        info = self._lookup_func_defaults(module_name, func_id)
        if info is None:
            return args
        total_params = info.get("params")
        defaults = info.get("defaults", [])
        func_obj = None
        if total_params is not None:
            missing = total_params - len(args)
            if missing > 0 and any(
                not spec.get("const", False) for spec in defaults[-missing:]
            ):
                resolved_module = module_name or self.module_name
                normalized = self._normalize_allowlist_module(resolved_module)
                if normalized is not None:
                    resolved_module = normalized
                if resolved_module == self.module_name:
                    func_obj = self._emit_module_attr_get(func_id)
                else:
                    func_obj = self._emit_module_attr_get_on(resolved_module, func_id)
        return self._apply_default_specs(
            total_params,
            defaults,
            args,
            node,
            call_name=func_id,
            func_obj=func_obj,
        )

    def _emit_direct_call_args(
        self, module_name: str | None, func_id: str, node: ast.Call
    ) -> list[MoltValue]:
        if node.keywords:
            raise NotImplementedError("Call keywords are not supported")
        args = self._emit_call_args(node.args)
        return self._apply_direct_call_defaults(module_name, func_id, args, node)

    def _emit_direct_call_args_for_symbol(
        self,
        func_symbol: str,
        node: ast.Call,
        func_obj: MoltValue | None = None,
    ) -> tuple[list[MoltValue], MoltValue | None]:
        if node.keywords:
            raise NotImplementedError("Call keywords are not supported")
        args = self._emit_call_args(node.args)
        info = self.func_default_specs.get(func_symbol)
        if info is None:
            func_name = self.func_symbol_names.get(func_symbol)
            if func_name is not None:
                info = self._lookup_func_defaults(None, func_name)
        if info is None:
            return args, func_obj
        total_params = info.get("params")
        defaults = info.get("defaults", [])
        if total_params is not None:
            missing = total_params - len(args)
            if missing > 0 and any(
                not spec.get("const", False) for spec in defaults[-missing:]
            ):
                if func_obj is None:
                    func_obj = self.visit(node.func)
        args = self._apply_default_specs(
            total_params,
            defaults,
            args,
            node,
            call_name=self.func_symbol_names.get(func_symbol, func_symbol),
            func_obj=func_obj,
        )
        return args, func_obj

    def _emit_dataclasses_field_call(
        self, module_name: str, node: ast.Call
    ) -> MoltValue:
        if any(kw.arg is None for kw in node.keywords):
            raise NotImplementedError("field does not support **kwargs")
        if len(node.args) > 2:
            raise NotImplementedError("field expects at most 2 arguments")
        default_expr: ast.expr | None = node.args[0] if node.args else None
        default_factory_expr: ast.expr | None = (
            node.args[1] if len(node.args) == 2 else None
        )
        for kw in node.keywords:
            if kw.arg == "default":
                if default_expr is not None:
                    raise NotImplementedError("field got multiple values for default")
                default_expr = kw.value
            elif kw.arg == "default_factory":
                if default_factory_expr is not None:
                    raise NotImplementedError(
                        "field got multiple values for default_factory"
                    )
                default_factory_expr = kw.value
            else:
                raise NotImplementedError(f"field got unexpected keyword {kw.arg}")
        if default_expr is None:
            default_val = self._emit_module_attr_get_on(module_name, "MISSING")
        else:
            default_val = self.visit(default_expr)
            if default_val is None:
                raise NotImplementedError("Unsupported field default")
        if default_factory_expr is None:
            default_factory_val = self._emit_module_attr_get_on(module_name, "MISSING")
        else:
            default_factory_val = self.visit(default_factory_expr)
            if default_factory_val is None:
                raise NotImplementedError("Unsupported field default_factory")
        res = MoltValue(self.next_var(), type_hint="Any")
        target_name = f"{self._sanitize_module_name(module_name)}__field"
        self.emit(
            MoltOp(
                kind="CALL",
                args=[target_name, default_val, default_factory_val],
                result=res,
            )
        )
        return res

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

    def _emit_exception_class(self, name: str) -> MoltValue:
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[name], result=kind_val))
        class_val = MoltValue(self.next_var(), type_hint="type")
        self.emit(MoltOp(kind="EXCEPTION_CLASS", args=[kind_val], result=class_val))
        return class_val

    def _emit_exception_new_from_args(
        self, kind: str, args: list[MoltValue]
    ) -> MoltValue:
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[kind], result=kind_val))
        args_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, args_val],
                result=exc_val,
            )
        )
        return exc_val

    def _emit_exception_new_from_class(
        self, class_val: MoltValue, args: list[MoltValue]
    ) -> MoltValue:
        args_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=args, result=args_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW_FROM_CLASS",
                args=[class_val, args_val],
                result=exc_val,
            )
        )
        return exc_val

    def _emit_exception_new(self, kind: str, message: str | MoltValue) -> MoltValue:
        args: list[MoltValue] = []
        if isinstance(message, MoltValue):
            if message.type_hint == "str":
                args = [message]
            else:
                args = [self._emit_str_from_obj(message)]
        elif message:
            msg_val = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[message], result=msg_val))
            args = [msg_val]
        return self._emit_exception_new_from_args(kind, args)

    def _emit_missing_value(self) -> MoltValue:
        missing = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="MISSING", args=[], result=missing))
        return missing

    def _emit_unbound_local_guard(self, value: MoltValue, name: str) -> None:
        missing = self._emit_missing_value()
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, missing], result=is_missing))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        msg = (
            "cannot access local variable "
            f"'{name}' where it is not associated with a value"
        )
        err_val = self._emit_exception_new("UnboundLocalError", msg)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_unbound_free_guard(self, value: MoltValue, name: str) -> None:
        missing = self._emit_missing_value()
        is_missing = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, missing], result=is_missing))
        self.emit(MoltOp(kind="IF", args=[is_missing], result=MoltValue("none")))
        msg = (
            "cannot access free variable "
            f"'{name}' where it is not associated with a value in enclosing scope"
        )
        err_val = self._emit_exception_new("NameError", msg)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_type_error_value(self, message: str, type_hint: str = "Any") -> MoltValue:
        err_val = self._emit_exception_new("TypeError", message)
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        res = MoltValue(self.next_var(), type_hint=type_hint)
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
        return res

    def _emit_stop_iteration_from_value(self, value: MoltValue) -> None:
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[value, none_val], result=is_none))
        args_cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[none_val], result=args_cell))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        empty_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=[], result=empty_tuple))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[args_cell, zero, empty_tuple],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        value_tuple = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_NEW", args=[value], result=value_tuple))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[args_cell, zero, value_tuple],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        args_val = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="INDEX", args=[args_cell, zero], result=args_val))
        kind_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["StopIteration"], result=kind_val))
        exc_val = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="EXCEPTION_NEW",
                args=[kind_val, args_val],
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
        class_val = self.visit(handler.type)
        if class_val is None:
            self._bridge_fallback(
                handler,
                "except (unsupported handler)",
                alternative="use a lowered exception name or tuple",
                detail="handler expression could not be lowered",
            )
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[0], result=res))
            return res
        res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="ISINSTANCE", args=[exc_val, class_val], result=res))
        return res

    def _apply_explicit_hint(self, name: str, value: MoltValue) -> None:
        hint = self.explicit_type_hints.get(name)
        if hint is None:
            return
        if self.type_hint_policy == "check":
            self._emit_guard_type(value, hint)
            self._apply_hint_to_value(name, value, hint)
            return
        if self.type_hint_policy == "trust" or self.stdlib_hint_trust:
            self._apply_hint_to_value(name, value, hint)

    def _emit_builtin_function(self, func_id: str) -> MoltValue:
        spec = BUILTIN_FUNC_SPECS[func_id]
        arity = len(spec.params) + len(spec.pos_or_kw_params) + len(spec.kwonly_params)
        if spec.vararg is not None:
            arity += 1
        func_val = MoltValue(self.next_var(), type_hint="function")
        self.emit(
            MoltOp(
                kind="BUILTIN_FUNC",
                args=[spec.runtime, arity],
                result=func_val,
            )
        )
        self._emit_function_metadata(
            func_val,
            name=func_id,
            qualname=func_id,
            posonly_params=list(spec.params),
            pos_or_kw_params=list(spec.pos_or_kw_params),
            kwonly_params=list(spec.kwonly_params),
            vararg=spec.vararg,
            varkw=None,
            default_exprs=list(spec.defaults),
            kw_default_exprs=list(spec.kw_defaults),
            docstring=None,
            bind_kind=MOLT_BIND_KIND_OPEN if func_id == "open" else None,
            module_override="builtins",
        )
        return func_val

    def visit_Name(self, node: ast.Name) -> Any:
        if isinstance(node.ctx, ast.Load):
            if node.id == "__molt_missing__":
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="MISSING", args=[], result=res))
                return res
            if node.id == "NotImplemented":
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="CONST_NOT_IMPLEMENTED", args=[], result=res))
                return res
            if node.id == "Ellipsis":
                res = MoltValue(self.next_var(), type_hint="ellipsis")
                self.emit(MoltOp(kind="CONST_ELLIPSIS", args=[], result=res))
                return res
            if node.id == "__name__":
                module_name = (
                    "__main__"
                    if self.entry_module and self.module_name == self.entry_module
                    else self.module_name
                )
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=res))
                return res
            if node.id in self.nonlocal_decls and node.id not in self.free_vars:
                raise NotImplementedError("nonlocal binding not found")
            if node.id in self.free_vars:
                free_val = self._emit_free_var_load(node.id)
                if free_val is not None:
                    return free_val
            if (
                self.current_func_name == "molt_main"
                and node.id in self.module_global_mutations
            ):
                return self._emit_module_attr_get(node.id)
            local = self._load_local_value(node.id)
            if local is not None:
                return local
            global_val = self.globals.get(node.id)
            if global_val is None:
                if node.id == "TYPE_CHECKING":
                    res = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[0], result=res))
                    return res
                if self.current_func_name != "molt_main" and (
                    node.id in self.module_declared_funcs
                    or node.id in self.module_declared_classes
                ):
                    return self._emit_global_get(node.id)
                builtin_tag = BUILTIN_TYPE_TAGS.get(node.id)
                if builtin_tag is not None:
                    tag_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[builtin_tag], result=tag_val))
                    res = MoltValue(self.next_var(), type_hint="type")
                    self.emit(MoltOp(kind="BUILTIN_TYPE", args=[tag_val], result=res))
                    return res
                if node.id in BUILTIN_FUNC_SPECS:
                    return self._emit_builtin_function(node.id)
                if node.id in BUILTIN_EXCEPTION_NAMES:
                    return self._emit_exception_class(node.id)
                if node.id in self.stdlib_allowlist:
                    module_val = self._emit_module_load(node.id)
                    if self.current_func_name == "molt_main":
                        self.globals[node.id] = module_val
                        self._emit_module_attr_set(node.id, module_val)
                    return module_val
                return self._emit_global_get(node.id)
            if self.current_func_name == "molt_main":
                return global_val
            return self._emit_global_get(node.id)
        return node.id

    def visit_Global(self, node: ast.Global) -> None:
        if self.current_func_name == "molt_main":
            return None
        self.global_decls.update(node.names)
        return None

    def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
        if self.current_func_name == "molt_main":
            raise NotImplementedError("nonlocal declarations at module scope")
        for name in node.names:
            if name in self.global_decls:
                raise NotImplementedError("nonlocal conflicts with global declaration")
        self.nonlocal_decls.update(node.names)
        return None

    def _box_local(self, name: str) -> None:
        if name in self.global_decls:
            return
        if name in self.boxed_locals:
            return
        if name in self.free_vars:
            cell = self._load_free_var_cell(name)
            if cell is None:
                return
            self.boxed_locals[name] = cell
            hint = self.free_var_hints.get(name)
            self.boxed_local_hints[name] = hint or "Any"
            self.locals[name] = cell
            return
        init: MoltValue
        if self.is_async() and name in self.async_locals:
            init = MoltValue(
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.async_locals[name]],
                    result=init,
                )
            )
        elif name in self.locals:
            init = self.locals[name]
        else:
            if name in self.scope_assigned or name in self.del_targets:
                init = self._emit_missing_value()
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
        if self.is_async():
            offset = self._async_local_offset(name)
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", offset, cell],
                    result=MoltValue("none"),
                )
            )

    def _load_boxed_cell(self, name: str) -> MoltValue | None:
        cell = self.boxed_locals.get(name)
        if cell is None:
            return None
        if not self.is_async():
            return cell
        if name not in self.async_locals:
            return cell
        slot_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", self.async_locals[name]],
                result=slot_val,
            )
        )
        return slot_val

    def _collect_assigned_names(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        class AssignCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                self.names.update(outer._collect_target_names(node.target))
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node.value)

            def visit_For(self, node: ast.For) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node)

            def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
                self.names.update(outer._collect_target_names(node.target))
                self.generic_visit(node)

            def visit_With(self, node: ast.With) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self.names.update(
                            outer._collect_target_names(item.optional_vars)
                        )
                self.generic_visit(node)

            def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
                for item in node.items:
                    if item.optional_vars is not None:
                        self.names.update(
                            outer._collect_target_names(item.optional_vars)
                        )
                self.generic_visit(node)

            def visit_If(self, node: ast.If) -> None:
                if outer._is_type_checking_test(node.test):
                    for stmt in node.orelse:
                        self.visit(stmt)
                    return None
                self.visit(node.test)
                for stmt in node.body:
                    self.visit(stmt)
                for stmt in node.orelse:
                    self.visit(stmt)

            def visit_ExceptHandler(self, node: ast.ExceptHandler) -> None:
                if node.name:
                    self.names.add(node.name)
                self.generic_visit(node)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                self.names.add(node.name)
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                self.names.add(node.name)
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                self.names.add(node.name)
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AssignCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    @staticmethod
    def _is_type_checking_test(expr: ast.expr) -> bool:
        if isinstance(expr, ast.Name):
            return expr.id == "TYPE_CHECKING"
        if isinstance(expr, ast.Attribute):
            if expr.attr != "TYPE_CHECKING":
                return False
            if isinstance(expr.value, ast.Name):
                return expr.value.id in {"typing", "typing_extensions"}
        return False

    def _collect_namedexpr_names(self, node: ast.AST) -> set[str]:
        names: set[str] = set()

        class NamedExprCollector(ast.NodeVisitor):
            def visit_NamedExpr(self, node: ast.NamedExpr) -> None:
                if isinstance(node.target, ast.Name):
                    names.add(node.target.id)
                self.generic_visit(node.value)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        NamedExprCollector().visit(node)
        return names

    def _collect_deleted_names(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        class DeleteCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    self.names.update(outer._collect_target_names(target))

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = DeleteCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_free_vars(
        self, node: ast.FunctionDef | ast.AsyncFunctionDef
    ) -> list[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names(node.body)
        global_decls = self._collect_global_decls(node.body)
        nonlocal_decls = self._collect_nonlocal_decls(node.body)
        local_names = params | (assigned - nonlocal_decls)
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for stmt in node.body:
            collector.visit(stmt)
        used.update(nonlocal_decls)
        used.update(self._collect_nested_free_vars(node.body))
        candidates = {
            name
            for name in used
            if name not in local_names and name not in global_decls
        }
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_free_vars_expr(self, node: ast.Lambda) -> list[str]:
        params = set(self._function_param_names(node.args))
        assigned = self._collect_assigned_names([ast.Expr(value=node.body)])
        local_names = params | assigned
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        Collector().visit(node.body)
        used.update(self._collect_nested_free_vars([node.body]))
        candidates = {name for name in used if name not in local_names}
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_free_vars_comprehension(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> list[str]:
        target_names: set[str] = set()
        exprs: list[ast.expr] = []
        for comp in node.generators:
            target_names.update(self._collect_target_names(comp.target))
            exprs.append(comp.iter)
            exprs.extend(comp.ifs)
        if isinstance(node, ast.DictComp):
            exprs.append(node.key)
            exprs.append(node.value)
        else:
            exprs.append(node.elt)
        namedexpr_targets: set[str] = set()
        for expr in exprs:
            namedexpr_targets |= self._collect_namedexpr_names(expr)
        assigned = self._collect_assigned_names(
            [ast.Expr(value=expr) for expr in exprs]
        )
        local_names = target_names | assigned
        used: set[str] = set()

        class Collector(ast.NodeVisitor):
            def visit_Name(self, node: ast.Name) -> Any:
                if isinstance(node.ctx, ast.Load):
                    used.add(node.id)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = Collector()
        for expr in exprs:
            collector.visit(expr)
        used |= namedexpr_targets
        used.update(self._collect_nested_free_vars(exprs))
        candidates = {name for name in used if name not in local_names}
        outer_scope = set(self.locals) | set(self.boxed_locals)
        if self.is_async():
            outer_scope |= set(self.async_locals)
        outer_scope |= set(self.free_vars) | self.scope_assigned
        return sorted(name for name in candidates if name in outer_scope)

    def _collect_nested_free_vars(self, nodes: Sequence[ast.AST]) -> set[str]:
        nested: set[str] = set()
        outer = self

        class NestedCollector(ast.NodeVisitor):
            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                nested.update(outer._collect_free_vars(node))
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                nested.update(outer._collect_free_vars(node))
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                nested.update(outer._collect_free_vars_expr(node))
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        collector = NestedCollector()
        for node in nodes:
            collector.visit(node)
        return nested

    def _collect_namedexpr_targets_comprehension(
        self, node: ast.GeneratorExp | ast.ListComp | ast.SetComp | ast.DictComp
    ) -> set[str]:
        target_names: set[str] = set()
        exprs: list[ast.expr] = []
        for comp in node.generators:
            target_names.update(self._collect_target_names(comp.target))
            exprs.append(comp.iter)
            exprs.extend(comp.ifs)
        if isinstance(node, ast.DictComp):
            exprs.append(node.key)
            exprs.append(node.value)
        else:
            exprs.append(node.elt)
        names: set[str] = set()
        for expr in exprs:
            names |= self._collect_namedexpr_names(expr)
        names -= target_names
        return names

    def _emit_free_var_load(self, name: str) -> MoltValue | None:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return None
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        hint = self.free_var_hints.get(name, "Any")
        res = MoltValue(self.next_var(), type_hint=hint)
        self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=res))
        self._emit_unbound_free_guard(res, name)
        return res

    def _emit_free_var_store(self, name: str, value: MoltValue) -> bool:
        cell = self._load_free_var_cell(name)
        if cell is None:
            return False
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, zero, value],
                result=MoltValue("none"),
            )
        )
        return True

    def _load_free_var_cell(self, name: str) -> MoltValue | None:
        closure = self.locals.get(_MOLT_CLOSURE_PARAM)
        if (
            closure is None
            and self.is_async()
            and self.async_closure_offset is not None
        ):
            closure = MoltValue(self.next_var(), type_hint="tuple")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", self.async_closure_offset],
                    result=closure,
                )
            )
        if closure is None:
            return None
        idx = self.free_vars.get(name)
        if idx is None:
            return None
        idx_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="INDEX", args=[closure, idx_val], result=cell))
        return cell

    def _collect_class_mutations(self, nodes: list[ast.stmt]) -> set[str]:
        outer = self

        def record_target(target: ast.AST, names: set[str]) -> None:
            if isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name):
                class_name = target.value.id
                if class_name in outer.classes:
                    names.add(class_name)
            elif isinstance(target, ast.Starred):
                record_target(target.value, names)
            elif isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    record_target(elt, names)

        class ClassMutationCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Assign(self, node: ast.Assign) -> None:
                for target in node.targets:
                    record_target(target, self.names)
                self.generic_visit(node.value)

            def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                record_target(node.target, self.names)
                if node.value is not None:
                    self.generic_visit(node.value)

            def visit_AugAssign(self, node: ast.AugAssign) -> None:
                record_target(node.target, self.names)
                self.generic_visit(node.value)

            def visit_Delete(self, node: ast.Delete) -> None:
                for target in node.targets:
                    record_target(target, self.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = ClassMutationCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _collect_loop_guard_candidates(self, body: list[ast.stmt]) -> dict[str, str]:
        if self.is_async():
            return {}
        assigned = self._collect_assigned_names(body)
        mutated_classes = self._collect_class_mutations(body)
        attr_names: set[str] = set()

        class AttrCollector(ast.NodeVisitor):
            def visit_Attribute(self, node: ast.Attribute) -> None:
                if isinstance(node.value, ast.Name) and isinstance(node.ctx, ast.Load):
                    attr_names.add(node.value.id)
                self.generic_visit(node)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = AttrCollector()
        for stmt in body:
            collector.visit(stmt)
        candidates: dict[str, str] = {}
        for name in sorted(attr_names):
            if name in assigned:
                continue
            expected_class = self.exact_locals.get(name)
            if expected_class is None:
                continue
            if expected_class in mutated_classes:
                continue
            candidates[name] = expected_class
        return candidates

    def _collect_target_names(self, target: ast.AST) -> set[str]:
        if isinstance(target, ast.Name):
            return {target.id}
        if isinstance(target, ast.Starred):
            return self._collect_target_names(target.value)
        if isinstance(target, (ast.Tuple, ast.List)):
            names: set[str] = set()
            for elt in target.elts:
                names.update(self._collect_target_names(elt))
            return names
        return set()

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

    def _collect_nonlocal_decls(self, nodes: list[ast.stmt]) -> set[str]:
        class NonlocalCollector(ast.NodeVisitor):
            def __init__(self) -> None:
                self.names: set[str] = set()

            def visit_Nonlocal(self, node: ast.Nonlocal) -> None:
                self.names.update(node.names)

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

        collector = NonlocalCollector()
        for stmt in nodes:
            collector.visit(stmt)
        return collector.names

    def _class_id_from_call(self, node: ast.Call) -> str | None:
        if isinstance(node.func, ast.Name) and node.func.id in self.classes:
            return node.func.id
        return None

    def _update_exact_local(self, name: str, value: ast.AST | None) -> None:
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

    def _propagate_func_type_hint(
        self, value_node: MoltValue, source_expr: ast.AST | None
    ) -> None:
        if not isinstance(source_expr, ast.Name):
            return
        source_info = self.locals.get(source_expr.id) or self.globals.get(
            source_expr.id
        )
        if source_info is None:
            return
        hint = source_info.type_hint
        if not isinstance(hint, str):
            return
        if hint.startswith(
            ("AsyncFunc:", "AsyncClosureFunc:", "GenFunc:", "GenClosureFunc:")
        ):
            symbol = hint.split(":")[1]
            base_symbol = (
                symbol[: -len("_poll")] if symbol.endswith("_poll") else symbol
            )
            if base_symbol in self.func_default_specs:
                value_node.type_hint = hint
            return
        if hint.startswith("Func:"):
            symbol = hint.split(":")[1]
            if symbol in self.func_default_specs:
                value_node.type_hint = hint

    def _load_local_value(self, name: str) -> MoltValue | None:
        if self.current_func_name != "molt_main" and name in self.global_decls:
            return self._emit_module_attr_get(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            res = MoltValue(self.next_var())
            hint = self.boxed_local_hints.get(name)
            if hint is not None:
                res.type_hint = hint
            self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
            if name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        if self.is_async() and name in self.async_locals:
            offset = self.async_locals[name]
            res = MoltValue(
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
            if name in self.unbound_check_names:
                self._emit_unbound_local_guard(res, name)
            return res
        return self.locals.get(name)

    def _load_local_value_unchecked(self, name: str) -> MoltValue | None:
        if self.current_func_name != "molt_main" and name in self.global_decls:
            return None
        cell = self._load_boxed_cell(name)
        if cell is not None:
            idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=idx))
            res = MoltValue(self.next_var())
            hint = self.boxed_local_hints.get(name)
            if hint is not None:
                res.type_hint = hint
            self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=res))
            return res
        if self.is_async() and name in self.async_locals:
            offset = self.async_locals[name]
            res = MoltValue(
                self.next_var(), type_hint=self.async_local_hints.get(name, "Any")
            )
            self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", offset], result=res))
            return res
        return self.locals.get(name)

    def _store_local_value(self, name: str, value: MoltValue) -> None:
        self._invalidate_loop_guard(name)
        if self.current_func_name != "molt_main" and name in self.global_decls:
            self._emit_module_attr_set_runtime(name, value)
            return
        if name in self.nonlocal_decls and name not in self.free_vars:
            raise NotImplementedError("nonlocal binding not found")
        if name in self.free_vars or name in self.nonlocal_decls:
            if self._emit_free_var_store(name, value):
                return
        if self.control_flow_depth == 0 and name in self.unbound_check_names:
            self.unbound_check_names.discard(name)
        cell = self._load_boxed_cell(name)
        if cell is not None:
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

    def _expr_needs_async(self, node: ast.AST) -> bool:
        class AsyncVisitor(ast.NodeVisitor):
            def __init__(self) -> None:
                self.needs_async = False

            def visit_Await(self, node: ast.Await) -> None:
                self.needs_async = True

            def visit_Call(self, node: ast.Call) -> None:
                if isinstance(node.func, ast.Name) and node.func.id in {
                    "molt_chan_send",
                    "molt_chan_recv",
                }:
                    self.needs_async = True
                    return
                self.generic_visit(node)

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        visitor = AsyncVisitor()
        visitor.visit(node)
        return visitor.needs_async

    def _expr_contains_yield(self, node: ast.AST) -> bool:
        class YieldVisitor(ast.NodeVisitor):
            def __init__(self) -> None:
                self.found = False

            def visit_Yield(self, node: ast.Yield) -> None:
                self.found = True

            def visit_YieldFrom(self, node: ast.YieldFrom) -> None:
                self.found = True

            def visit_Lambda(self, node: ast.Lambda) -> None:
                return

            def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                return

            def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
                return

            def visit_ClassDef(self, node: ast.ClassDef) -> None:
                return

        visitor = YieldVisitor()
        visitor.visit(node)
        return visitor.found

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

    def _maybe_spill_receiver(
        self, receiver: MoltValue, args: list[ast.expr]
    ) -> tuple[MoltValue, int | None]:
        if not self.is_async() or not args:
            return receiver, None
        if not any(self._expr_may_yield(arg) for arg in args):
            return receiver, None
        slot = self._spill_async_value(
            receiver, f"__recv_spill_{len(self.async_locals)}"
        )
        return receiver, slot

    def _spill_async_temporaries(self) -> None:
        # TODO(perf, owner:compiler, milestone:TC2, priority:P1, status:planned): narrow spill set
        # using a CFG liveness pass to avoid redundant closure traffic across state labels.
        label_indices = [
            idx for idx, op in enumerate(self.current_ops) if op.kind == "STATE_LABEL"
        ]
        if not label_indices:
            return
        params = set(self.funcs_map[self.current_func_name]["params"])
        last_use: dict[str, int] = {}
        first_def: dict[str, int] = {name: -1 for name in params}
        type_hints: dict[str, str] = {}
        for idx, op in enumerate(self.current_ops):
            out_name = op.result.name
            if out_name != "none":
                first_def.setdefault(out_name, idx)
                if op.result.type_hint:
                    type_hints[out_name] = op.result.type_hint
            for arg in op.args:
                if isinstance(arg, MoltValue):
                    last_use[arg.name] = idx

        label_spills: dict[int, set[str]] = {idx: set() for idx in label_indices}
        spill_names: set[str] = set()
        for name, last_idx in last_use.items():
            if name == "self":
                continue
            first_idx = first_def.get(name)
            if first_idx is None:
                continue
            for label_idx in label_indices:
                if first_idx < label_idx < last_idx:
                    label_spills[label_idx].add(name)
                    spill_names.add(name)
        if not spill_names:
            return
        for name in spill_names:
            offset = self._async_local_offset(name)
            hint = type_hints.get(name)
            if hint is not None:
                self.async_local_hints.setdefault(name, hint)

        new_ops: list[MoltOp] = []
        for idx, op in enumerate(self.current_ops):
            new_ops.append(op)
            if op.kind == "STATE_LABEL":
                for name in sorted(label_spills.get(idx, set())):
                    offset = self.async_locals[name]
                    hint = type_hints.get(name, "Unknown")
                    new_ops.append(
                        MoltOp(
                            kind="LOAD_CLOSURE",
                            args=["self", offset],
                            result=MoltValue(name, type_hint=hint),
                        )
                    )
                continue
            out_name = op.result.name
            if (
                out_name != "none"
                and out_name in spill_names
                and op.kind != "LOAD_CLOSURE"
            ):
                offset = self.async_locals[out_name]
                new_ops.append(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=[
                            "self",
                            offset,
                            MoltValue(out_name, type_hint=op.result.type_hint),
                        ],
                        result=MoltValue("none"),
                    )
                )
        self.current_ops[:] = new_ops

    def _active_exception_value(self, exc: ActiveException) -> MoltValue:
        if self.is_async() and exc.slot is not None:
            return self._reload_async_value(exc.slot, exc.value.type_hint)
        return exc.value

    def _emit_expr_list(self, exprs: list[ast.expr]) -> list[MoltValue]:
        if not exprs:
            return []
        if not self.is_async():
            values: list[MoltValue] = []
            for expr in exprs:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported expression")
                values.append(val)
            return values
        yield_flags = [self._expr_may_yield(expr) for expr in exprs]
        if not any(yield_flags):
            values = []
            for expr in exprs:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported expression")
                values.append(val)
            return values
        values = []
        spills: list[tuple[int, int, str]] = []
        for idx, expr in enumerate(exprs):
            val = self.visit(expr)
            if val is None:
                raise NotImplementedError("Unsupported expression")
            values.append(val)
            if any(yield_flags[idx + 1 :]):
                slot = self._spill_async_value(
                    val, f"__expr_spill_{len(self.async_locals)}"
                )
                spills.append((idx, slot, val.type_hint))
        for idx, slot, hint in spills:
            values[idx] = self._reload_async_value(slot, hint)
        return values

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

    @staticmethod
    def _call_needs_bind(node: ast.Call) -> bool:
        if node.keywords:
            return True
        return any(isinstance(arg, ast.Starred) for arg in node.args)

    def _emit_call_args_builder(self, node: ast.Call) -> MoltValue:
        items: list[tuple[str, ast.expr, str | None]] = []
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                items.append(("star", arg.value, None))
            else:
                items.append(("pos", arg, None))
        for kw in node.keywords:
            if kw.arg is None:
                items.append(("kwstar", kw.value, None))
            else:
                items.append(("kw", kw.value, kw.arg))
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        if not items:
            return callargs
        values: list[MoltValue] = []
        if not self.is_async():
            for _, expr, _ in items:
                val = self.visit(expr)
                if val is None:
                    raise NotImplementedError("Unsupported call argument")
                values.append(val)
        else:
            yield_flags = [self._expr_may_yield(expr) for _, expr, _ in items]
            if not any(yield_flags):
                for _, expr, _ in items:
                    val = self.visit(expr)
                    if val is None:
                        raise NotImplementedError("Unsupported call argument")
                    values.append(val)
            else:
                spills: list[tuple[int, int, str]] = []
                for idx, (_, expr, _) in enumerate(items):
                    val = self.visit(expr)
                    if val is None:
                        raise NotImplementedError("Unsupported call argument")
                    values.append(val)
                    if any(yield_flags[idx + 1 :]):
                        slot = self._spill_async_value(
                            val, f"__arg_spill_{len(self.async_locals)}"
                        )
                        spills.append((idx, slot, val.type_hint))
                for idx, slot, hint in spills:
                    values[idx] = self._reload_async_value(slot, hint)
        for (kind, _, name), val in zip(items, values):
            if kind == "pos":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="CALLARGS_PUSH_POS", args=[callargs, val], result=res)
                )
            elif kind == "star":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_STAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            elif kind == "kw":
                if name is None:
                    raise NotImplementedError("Keyword name is missing")
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_KW",
                        args=[callargs, key_val, val],
                        result=res,
                    )
                )
            elif kind == "kwstar":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_KWSTAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            else:
                raise NotImplementedError("Unknown call argument kind")
        return callargs

    def _emit_print_call_args_builder(self, node: ast.Call) -> tuple[MoltValue, bool]:
        items: list[tuple[str, ast.expr, str | None]] = []
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                items.append(("star", arg.value, None))
            else:
                items.append(("pos", arg, None))
        for kw in node.keywords:
            if kw.arg is None:
                items.append(("kwstar", kw.value, None))
            else:
                items.append(("kw", kw.value, kw.arg))
        callargs = MoltValue(self.next_var(), type_hint="callargs")
        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
        if not items:
            return callargs, False
        values: list[MoltValue] = []
        saw_name_error = False
        if not self.is_async():
            for _, expr, _ in items:
                val = self.visit(expr)
                if val is None:
                    if isinstance(expr, ast.Name):
                        exc_val = self._emit_exception_new(
                            "NameError", f"name '{expr.id}' is not defined"
                        )
                        self.emit(
                            MoltOp(
                                kind="RAISE",
                                args=[exc_val],
                                result=MoltValue("none"),
                            )
                        )
                        saw_name_error = True
                        val = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                    else:
                        raise NotImplementedError("Unsupported call argument")
                values.append(val)
        else:
            yield_flags = [self._expr_may_yield(expr) for _, expr, _ in items]
            if not any(yield_flags):
                for _, expr, _ in items:
                    val = self.visit(expr)
                    if val is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    values.append(val)
            else:
                spills: list[tuple[int, int, str]] = []
                for idx, (_, expr, _) in enumerate(items):
                    val = self.visit(expr)
                    if val is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            val = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    values.append(val)
                    if any(yield_flags[idx + 1 :]):
                        slot = self._spill_async_value(
                            val, f"__arg_spill_{len(self.async_locals)}"
                        )
                        spills.append((idx, slot, val.type_hint))
                for idx, slot, hint in spills:
                    values[idx] = self._reload_async_value(slot, hint)
        for (kind, _, name), val in zip(items, values):
            if kind == "pos":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="CALLARGS_PUSH_POS", args=[callargs, val], result=res)
                )
            elif kind == "star":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_STAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            elif kind == "kw":
                if name is None:
                    raise NotImplementedError("Keyword name is missing")
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_KW",
                        args=[callargs, key_val, val],
                        result=res,
                    )
                )
            elif kind == "kwstar":
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_EXPAND_KWSTAR",
                        args=[callargs, val],
                        result=res,
                    )
                )
            else:
                raise NotImplementedError("Unknown call argument kind")
        return callargs, saw_name_error

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

    def _match_iter_vector_reduction_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        item_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Name):
            return None
        seq_name = node.iter.id
        stmt = node.body[0]
        if isinstance(stmt, ast.AugAssign):
            if not isinstance(stmt.op, (ast.Add, ast.Mult)):
                return None
            if not isinstance(stmt.target, ast.Name):
                return None
            if stmt.target.id == item_name:
                return None
            if isinstance(stmt.value, ast.Name) and stmt.value.id == item_name:
                kind = "sum" if isinstance(stmt.op, ast.Add) else "prod"
                return (stmt.target.id, seq_name, kind, None)
        if isinstance(stmt, ast.Assign):
            if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
                return None
            acc_name = stmt.targets[0].id
            if acc_name == item_name:
                return None
            if not isinstance(stmt.value, ast.BinOp) or not isinstance(
                stmt.value.op, (ast.Add, ast.Mult)
            ):
                return None
            left = stmt.value.left
            right = stmt.value.right
            if isinstance(left, ast.Name) and left.id == acc_name:
                if isinstance(right, ast.Name) and right.id == item_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (acc_name, seq_name, kind, None)
            if isinstance(right, ast.Name) and right.id == acc_name:
                if isinstance(left, ast.Name) and left.id == item_name:
                    kind = "sum" if isinstance(stmt.value.op, ast.Add) else "prod"
                    return (acc_name, seq_name, kind, None)
        return None

    def _match_iter_vector_minmax_loop(
        self, node: ast.For
    ) -> tuple[str, str, str, ast.expr | None] | None:
        if not isinstance(node.target, ast.Name):
            return None
        item_name = node.target.id
        if len(node.body) != 1:
            return None
        if not isinstance(node.iter, ast.Name):
            return None
        seq_name = node.iter.id
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
        left_is_acc = isinstance(left, ast.Name) and left.id == acc_name
        right_is_acc = isinstance(right, ast.Name) and right.id == acc_name
        left_is_item = isinstance(left, ast.Name) and left.id == item_name
        right_is_item = isinstance(right, ast.Name) and right.id == item_name
        if not ((left_is_acc and right_is_item) or (left_is_item and right_is_acc)):
            return None
        if isinstance(op, ast.Lt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "min", None
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "max", None
        if isinstance(op, ast.Gt):
            if left_is_item and right_is_acc:
                return acc_name, seq_name, "max", None
            if left_is_acc and right_is_item:
                return acc_name, seq_name, "min", None
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

    def _emit_iter_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
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
            guard_map = self._emit_hoisted_loop_guards(node.body)
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
            pair = self._emit_iter_next_checked(iter_val)
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
            self._emit_assign_target(target, item, None)
            self._visit_loop_body(node.body, guard_map, loop_break_flag=loop_break_flag)
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            return
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_loop_body() -> None:
            iter_obj = self._emit_iter_new(iterable)
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))

            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            pair = self._emit_iter_next_checked(iter_obj)
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
            self._emit_assign_target(target, item, None)
            self._visit_loop_body(node.body, None, loop_break_flag=loop_break_flag)
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return

        emit_loop_body()

    def _emit_index_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
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
            guard_map = self._emit_hoisted_loop_guards(node.body)
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
            self._emit_assign_target(target, item, None)
            self.async_index_loop_stack.append(idx_slot)
            self._visit_loop_body(node.body, guard_map, loop_break_flag=loop_break_flag)
            self.async_index_loop_stack.pop()
            idx_after = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", idx_slot],
                    result=idx_after,
                )
            )
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx_after, one], result=next_idx))
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
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_loop_body() -> None:
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
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            item = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[iterable, idx], result=item))
            self._emit_assign_target(target, item, None)
            self.range_loop_stack.append((idx, one))
            self._visit_loop_body(node.body, None, loop_break_flag=loop_break_flag)
            self.range_loop_stack.pop()
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, one], result=next_idx))
            self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return

        emit_loop_body()

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
            if stop is None:
                return None
            step = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step))
            return start, stop, step
        if len(node.args) == 2:
            start = self.visit(node.args[0])
            if start is None:
                return None
            stop = self.visit(node.args[1])
            if stop is None:
                return None
            step = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=step))
            return start, stop, step
        if len(node.args) == 3:
            start = self.visit(node.args[0])
            if start is None:
                return None
            stop = self.visit(node.args[1])
            if stop is None:
                return None
            step = self.visit(node.args[2])
            if step is None:
                return None
            return start, stop, step
        raise NotImplementedError("range expects 1, 2, or 3 arguments")

    def _emit_range_loop(
        self,
        node: ast.For,
        start: MoltValue,
        stop: MoltValue,
        step: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        target = node.target
        if self.is_async():
            range_obj = MoltValue(self.next_var(), type_hint="range")
            self.emit(
                MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=range_obj)
            )
            self._emit_iter_loop(node, range_obj, loop_break_flag=loop_break_flag)
            return None
        step_const = self.const_ints.get(step.name)
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_range_loop_body() -> None:
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
                        kind="LOOP_BREAK_IF_FALSE",
                        args=[cond],
                        result=MoltValue("none"),
                    )
                )
                self._emit_assign_target(target, idx, None)
                self.range_loop_stack.append((idx, step))
                self._visit_loop_body(node.body, None, loop_break_flag=loop_break_flag)
                self.range_loop_stack.pop()
                next_idx = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
                self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=idx))
                self.emit(
                    MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none"))
                )
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
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            self._emit_assign_target(target, idx, None)
            self.range_loop_stack.append((idx, step))
            self._visit_loop_body(node.body, None, loop_break_flag=loop_break_flag)
            self.range_loop_stack.pop()
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
            self._emit_assign_target(target, idx_neg, None)
            self.range_loop_stack.append((idx_neg, step))
            self._visit_loop_body(node.body, None, loop_break_flag=loop_break_flag)
            self.range_loop_stack.pop()
            next_idx_neg = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx_neg, step], result=next_idx_neg))
            self.emit(
                MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx_neg], result=idx_neg)
            )
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_range_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_range_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            return None

        emit_range_loop_body()
        return None

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
        pair = self._emit_iter_next_checked(iter_obj)
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

    def _emit_list_from_aiter(self, iterable: MoltValue) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("async list comprehension outside async context")
        res = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
        res_slot = self._async_local_offset(
            f"__async_list_comp_res_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_list_comp_iter_{len(self.async_locals)}"
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
            f"__async_list_comp_sentinel_{len(self.async_locals)}"
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
        res_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_val,
            )
        )
        self.emit(
            MoltOp(
                kind="LIST_APPEND",
                args=[res_val, item_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        res_final = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_final,
            )
        )
        return res_final

    def _emit_tuple_from_iter(self, iterable: MoltValue) -> MoltValue:
        items = self._emit_list_from_iter(iterable)
        res = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="TUPLE_FROM_LIST", args=[items], result=res))
        return res

    def _emit_set_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
        iter_obj = self._emit_iter_new(iterable)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(MoltOp(kind="SET_ADD", args=[res, item], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _emit_set_from_aiter(self, iterable: MoltValue) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("async set comprehension outside async context")
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
        res_slot = self._async_local_offset(
            f"__async_set_comp_res_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", res_slot, res],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_set_comp_iter_{len(self.async_locals)}"
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
            f"__async_set_comp_sentinel_{len(self.async_locals)}"
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
        res_val = MoltValue(self.next_var(), type_hint="set")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_val,
            )
        )
        self.emit(
            MoltOp(
                kind="SET_ADD",
                args=[res_val, item_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        res_final = MoltValue(self.next_var(), type_hint="set")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", res_slot],
                result=res_final,
            )
        )
        return res_final

    def _emit_dict_fill_from_iter(self, target: MoltValue, iterable: MoltValue) -> None:
        iter_obj = self._emit_iter_new(iterable)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        key = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item, zero], result=key))
        val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item, one], result=val))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[target, key, val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_dict_fill_from_aiter(
        self, target: MoltValue, iterable: MoltValue
    ) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("async dict comprehension outside async context")
        target_slot = self._async_local_offset(
            f"__async_dict_comp_target_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", target_slot, target],
                result=MoltValue("none"),
            )
        )
        iter_obj = self._emit_aiter(iterable)
        iter_slot = self._async_local_offset(
            f"__async_dict_comp_iter_{len(self.async_locals)}"
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
            f"__async_dict_comp_sentinel_{len(self.async_locals)}"
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
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        key = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item_val, zero], result=key))
        val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[item_val, one], result=val))
        target_val = MoltValue(self.next_var(), type_hint=target.type_hint or "dict")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", target_slot],
                result=target_val,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[target_val, key, val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        target_final = MoltValue(self.next_var(), type_hint=target.type_hint or "dict")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", target_slot],
                result=target_final,
            )
        )
        return target_final

    def _emit_set_update_from_iter(
        self, target: MoltValue, iterable: MoltValue
    ) -> None:
        iter_obj = self._emit_iter_new(iterable)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(MoltOp(kind="SET_ADD", args=[target, item], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

    def _emit_frozenset_from_iter(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="frozenset")
        self.emit(MoltOp(kind="FROZENSET_NEW", args=[], result=res))
        iter_obj = self._emit_iter_new(iterable)
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        one = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[1], result=one))
        self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
        pair = self._emit_iter_next_checked(iter_obj)
        done = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_TRUE", args=[done], result=MoltValue("none"))
        )
        item = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
        self.emit(
            MoltOp(kind="FROZENSET_ADD", args=[res, item], result=MoltValue("none"))
        )
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        return res

    def _emit_intarray_from_seq(self, seq: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="intarray")
        self.emit(MoltOp(kind="INTARRAY_FROM_SEQ", args=[seq], result=res))
        self.container_elem_hints[res.name] = "int"
        return res

    def _emit_iter_new(self, iterable: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="iter")
        self.emit(MoltOp(kind="ITER_NEW", args=[iterable], result=res))
        if self.try_end_labels:
            self._emit_raise_if_pending()
        else:
            self._emit_raise_if_pending(emit_exit=True)
        none_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
        is_none = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[res, none_val], result=is_none))
        self.emit(MoltOp(kind="IF", args=[is_none], result=MoltValue("none")))
        err_val = self._emit_exception_new("TypeError", "object is not iterable")
        self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return res

    def _emit_iter_next_checked(self, iter_obj: MoltValue) -> MoltValue:
        pair = MoltValue(self.next_var(), type_hint="tuple")
        self.emit(MoltOp(kind="ITER_NEXT", args=[iter_obj], result=pair))
        if not self.try_end_labels:
            self._emit_raise_if_pending(emit_exit=True)
        return pair

    def _emit_guarded_setattr(
        self,
        obj: MoltValue,
        attr: str,
        value: MoltValue,
        expected_class: str,
        *,
        use_init: bool = False,
        assume_exact: bool = False,
        obj_name: str | None = None,
    ) -> None:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        if class_info and self._class_is_exception_subclass(expected_class, class_info):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and attr not in class_info.get("fields", {}):
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if class_info and not class_info.get("static"):
            # TODO(perf, owner:frontend, milestone:TC1, priority:P2, status:planned): use a
            # captured class ref to enable guarded field access for local classes.
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
            self.emit(
                MoltOp(
                    kind=setattr_kind,
                    args=[obj, attr, value, expected_class],
                    result=MoltValue("none"),
                )
            )
            return
        if assumption is False:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_PTR",
                    args=[obj, attr, value],
                    result=MoltValue("none"),
                )
            )
            return
        if self._class_layout_stable(expected_class):
            if assume_exact or self.exact_locals.get(name) == expected_class:
                setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
                self.emit(
                    MoltOp(
                        kind=setattr_kind,
                        args=[obj, attr, value, expected_class],
                        result=MoltValue("none"),
                    )
                )
                return
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            class_ref = self._emit_class_ref(expected_class)
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            setattr_kind = "GUARDED_SETATTR_INIT" if use_init else "GUARDED_SETATTR"
            self.emit(
                MoltOp(
                    kind=setattr_kind,
                    args=[
                        obj,
                        class_ref,
                        expected_version,
                        attr,
                        value,
                        expected_class,
                    ],
                    result=MoltValue("none"),
                )
            )
            return

        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        setattr_kind = "SETATTR_INIT" if use_init else "SETATTR"
        self.emit(
            MoltOp(
                kind=setattr_kind,
                args=[obj, attr, value, expected_class],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_PTR",
                args=[obj, attr, value],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_guarded_getattr(
        self,
        obj: MoltValue,
        attr: str,
        expected_class: str,
        *,
        assume_exact: bool = False,
        obj_name: str | None = None,
    ) -> MoltValue:
        name = obj_name or obj.name
        class_info = self.classes.get(expected_class)
        if class_info and self._class_is_exception_subclass(expected_class, class_info):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if class_info and attr not in class_info.get("fields", {}):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if class_info and not class_info.get("static"):
            # TODO(perf, owner:frontend, milestone:TC1, priority:P2, status:planned): use a
            # captured class ref to enable guarded field access for local classes.
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        assumption = self._loop_guard_assumption(name, expected_class)
        if assumption is True:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, attr, expected_class],
                    result=res,
                )
            )
            return res
        if assumption is False:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=res,
                )
            )
            return res
        if self._class_layout_stable(expected_class):
            if assume_exact or self.exact_locals.get(name) == expected_class:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR",
                        args=[obj, attr, expected_class],
                        result=res,
                    )
                )
                return res
        guard = self._loop_guard_for(obj, expected_class, obj_name=name)
        if guard is None:
            class_ref = self._emit_class_ref(expected_class)
            expected_version = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="CONST",
                    args=[self.classes[expected_class].get("layout_version", 0)],
                    result=expected_version,
                )
            )
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GUARDED_GETATTR",
                    args=[obj, class_ref, expected_version, attr, expected_class],
                    result=res,
                )
            )
            return res
        return self._emit_guarded_field_get_with_guard(
            obj,
            fast_attr=attr,
            fallback_attr=attr,
            expected_class=expected_class,
            guard=guard,
        )

    def _emit_layout_guard(self, obj: MoltValue, expected_class: str) -> MoltValue:
        class_info = self.classes.get(expected_class)
        if class_info and not class_info.get("static"):
            guard = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=guard))
            return guard
        class_ref = self._emit_class_ref(expected_class)
        expected_version = MoltValue(self.next_var(), type_hint="int")
        self.emit(
            MoltOp(
                kind="CONST",
                args=[self.classes[expected_class].get("layout_version", 0)],
                result=expected_version,
            )
        )
        guard = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(
                kind="GUARD_LAYOUT",
                args=[obj, class_ref, expected_version],
                result=guard,
            )
        )
        return guard

    def _loop_guard_assumption(self, obj_name: str, expected_class: str) -> bool | None:
        for guard_map in reversed(self.loop_guard_assumptions):
            entry = guard_map.get(obj_name)
            if entry and entry[0] == expected_class:
                return entry[1]
        return None

    def _push_loop_guard_assumptions(
        self,
        guard_map: dict[str, tuple[str, MoltValue]],
        assume_true: bool,
    ) -> None:
        assumptions: dict[str, tuple[str, bool]] = {}
        for name, (expected_class, _) in guard_map.items():
            assumptions[name] = (expected_class, assume_true)
        self.loop_guard_assumptions.append(assumptions)

    def _pop_loop_guard_assumptions(self) -> None:
        if self.loop_guard_assumptions:
            self.loop_guard_assumptions.pop()

    def _loop_guard_for(
        self, obj: MoltValue, expected_class: str, *, obj_name: str | None = None
    ) -> MoltValue | None:
        if not self.loop_layout_guards:
            return None
        name = obj_name or obj.name
        if self.exact_locals.get(name) != expected_class:
            return None
        guard_map = self.loop_layout_guards[-1]
        cached = guard_map.get(name)
        if cached and cached[0] == expected_class:
            return cached[1]
        guard = self._emit_layout_guard(obj, expected_class)
        guard_map[name] = (expected_class, guard)
        return guard

    def _invalidate_loop_guard(self, name: str) -> None:
        for guard_map in self.loop_layout_guards:
            guard_map.pop(name, None)

    def _invalidate_loop_guards_for_class(self, class_name: str) -> None:
        for guard_map in self.loop_layout_guards:
            stale = [
                key for key, (klass, _) in guard_map.items() if klass == class_name
            ]
            for key in stale:
                guard_map.pop(key, None)

    def _emit_hoisted_loop_guards(
        self, body: list[ast.stmt]
    ) -> dict[str, tuple[str, MoltValue]]:
        if self.is_async():
            return {}
        candidates = self._collect_loop_guard_candidates(body)
        if not candidates:
            return {}
        guard_map: dict[str, tuple[str, MoltValue]] = {}
        for name, expected_class in sorted(candidates.items()):
            obj = self._load_local_value(name)
            if obj is None:
                obj = self.locals.get(name) or self.globals.get(name)
            if obj is None:
                continue
            guard = self._emit_layout_guard(obj, expected_class)
            guard_map[name] = (expected_class, guard)
        return guard_map

    def _emit_guard_map_condition(
        self, guard_map: dict[str, tuple[str, MoltValue]]
    ) -> MoltValue:
        condition: MoltValue | None = None
        for _, (_, guard) in sorted(guard_map.items()):
            if condition is None:
                condition = guard
                continue
            combined = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="AND", args=[condition, guard], result=combined))
            condition = combined
        if condition is None:
            condition = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=condition))
        return condition

    def _emit_guarded_field_get_with_guard(
        self,
        obj: MoltValue,
        fast_attr: str,
        fallback_attr: str,
        expected_class: str,
        guard: MoltValue,
    ) -> MoltValue:
        use_phi = self.enable_phi and not self.is_async()
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR",
                    args=[obj, fast_attr, expected_class],
                    result=fast_val,
                )
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, fallback_attr],
                    result=slow_val,
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = (
                fast_val.type_hint
                if fast_val.type_hint == slow_val.type_hint
                else "Any"
            )
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        placeholder = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[placeholder], result=cell))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var())
        self.emit(
            MoltOp(
                kind="GETATTR",
                args=[obj, fast_attr, expected_class],
                result=fast_val,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, fast_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, fallback_attr],
                result=slow_val,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, slow_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=merged))
        return merged

    def _emit_guarded_property_get(
        self,
        obj: MoltValue,
        attr: str,
        getter_symbol: str,
        expected_class: str,
        return_hint: str | None,
        *,
        obj_name: str | None = None,
    ) -> MoltValue:
        guard = self._loop_guard_for(obj, expected_class, obj_name=obj_name)
        if guard is None:
            guard = self._emit_layout_guard(obj, expected_class)
        use_phi = self.enable_phi and not self.is_async()
        fast_hint = return_hint or "Any"
        if use_phi:
            self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
            fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
            self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            slow_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, attr],
                    result=slow_val,
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            res_hint = fast_hint if fast_hint == slow_val.type_hint else "Any"
            merged = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="PHI", args=[fast_val, slow_val], result=merged))
            return merged

        placeholder = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[placeholder], result=cell))
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=idx))
        self.emit(MoltOp(kind="IF", args=[guard], result=MoltValue("none")))
        fast_val = MoltValue(self.next_var(), type_hint=fast_hint)
        self.emit(MoltOp(kind="CALL", args=[getter_symbol, obj], result=fast_val))
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, fast_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        slow_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_GENERIC_PTR",
                args=[obj, attr],
                result=slow_val,
            )
        )
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, idx, slow_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        merged = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[cell, idx], result=merged))
        return merged

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

    def _emit_for_loop(
        self,
        node: ast.For,
        iterable: MoltValue,
        loop_break_flag: int | str | None = None,
    ) -> None:
        if self._iterable_is_indexable(iterable):
            self._emit_index_loop(node, iterable, loop_break_flag=loop_break_flag)
        else:
            self._emit_iter_loop(node, iterable, loop_break_flag=loop_break_flag)

    def _emit_loop_orelse(self, break_name: str, orelse: list[ast.stmt]) -> None:
        break_val = self._load_local_value(break_name)
        if break_val is None:
            raise NotImplementedError("for-else break flag not initialized")
        should_run = self._emit_not(break_val)
        self.emit(MoltOp(kind="IF", args=[should_run], result=MoltValue("none")))
        self._visit_block(orelse)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

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

    def _match_const_increment(self, stmt: ast.stmt) -> tuple[str, int] | None:
        if isinstance(stmt, ast.AugAssign):
            if (
                isinstance(stmt.op, ast.Add)
                and isinstance(stmt.target, ast.Name)
                and isinstance(stmt.value, ast.Constant)
                and isinstance(stmt.value.value, int)
            ):
                return stmt.target.id, stmt.value.value
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
                and left.id == acc_name
                and isinstance(right, ast.Constant)
                and isinstance(right.value, int)
            ):
                return acc_name, right.value
            if (
                isinstance(right, ast.Name)
                and right.id == acc_name
                and isinstance(left, ast.Constant)
                and isinstance(left.value, int)
            ):
                return acc_name, left.value
        return None

    def _match_counted_while_const_increment(
        self, body: list[ast.stmt]
    ) -> tuple[str, int] | None:
        if len(body) == 1:
            return self._match_const_increment(body[0])
        if len(body) != 2:
            return None
        init, inner = body
        if not isinstance(init, ast.Assign):
            return None
        if len(init.targets) != 1 or not isinstance(init.targets[0], ast.Name):
            return None
        if not isinstance(init.value, ast.Constant) or not isinstance(
            init.value.value, int
        ):
            return None
        if not isinstance(inner, ast.While):
            return None
        inner_match = self._match_counted_while(inner)
        if inner_match is None:
            return None
        inner_index, inner_bound, inner_body = inner_match
        if inner_index != init.targets[0].id:
            return None
        inner_inc = self._match_counted_while_const_increment(inner_body)
        if inner_inc is None:
            return None
        acc_name, delta = inner_inc
        start_val = init.value.value
        if start_val >= inner_bound:
            return acc_name, 0
        return acc_name, (inner_bound - start_val) * delta

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
        guard_map = self._emit_hoisted_loop_guards(body)
        idx = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="LOOP_INDEX_START", args=[start], result=idx))
        cond = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[idx, stop], result=cond))
        self.emit(
            MoltOp(kind="LOOP_BREAK_IF_FALSE", args=[cond], result=MoltValue("none"))
        )
        self._store_local_value(index_name, idx)
        self._visit_loop_body(body, guard_map)
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
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
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
        elif isinstance(node.op, ast.Div):
            op_kind = "DIV"
            res_type = "float"
        elif isinstance(node.op, ast.FloorDiv):
            op_kind = "FLOORDIV"
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.Mod):
            op_kind = "MOD"
            if left.type_hint == right.type_hint == "int":
                res_type = "int"
            elif "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.Pow):
            op_kind = "POW"
            if "float" in {left.type_hint, right.type_hint}:
                res_type = "float"
        elif isinstance(node.op, ast.BitOr):
            op_kind = "BIT_OR"
            if left.type_hint == right.type_hint == "bool":
                res_type = "bool"
            elif {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
        elif isinstance(node.op, ast.BitAnd):
            op_kind = "BIT_AND"
            if left.type_hint == right.type_hint == "bool":
                res_type = "bool"
            elif {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
        elif isinstance(node.op, ast.BitXor):
            op_kind = "BIT_XOR"
            if left.type_hint == right.type_hint == "bool":
                res_type = "bool"
            elif {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
            elif left.type_hint in {"set", "frozenset"} and right.type_hint in {
                "set",
                "frozenset",
            }:
                res_type = left.type_hint
        elif isinstance(node.op, ast.LShift):
            op_kind = "LSHIFT"
            if {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
        elif isinstance(node.op, ast.RShift):
            op_kind = "RSHIFT"
            if {left.type_hint, right.type_hint}.issubset({"int", "bool"}):
                res_type = "int"
        elif isinstance(node.op, ast.MatMult):
            op_kind = "MATMUL"
            if left.type_hint == right.type_hint == "buffer2d":
                res_type = "buffer2d"
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
        if node.value is Ellipsis:
            res = MoltValue(self.next_var(), type_hint="ellipsis")
            self.emit(MoltOp(kind="CONST_ELLIPSIS", args=[], result=res))
            return res
        if isinstance(node.value, bool):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[node.value], result=res))
            return res
        if isinstance(node.value, int):
            inline_min = -(1 << 46)
            inline_max = (1 << 46) - 1
            res = MoltValue(self.next_var(), type_hint="int")
            if inline_min <= node.value <= inline_max:
                self.emit(MoltOp(kind="CONST", args=[node.value], result=res))
            else:
                self.emit(
                    MoltOp(kind="CONST_BIGINT", args=[str(node.value)], result=res)
                )
            return res
        if isinstance(node.value, bytes):
            res = MoltValue(self.next_var(), type_hint="bytes")
            self.emit(MoltOp(kind="CONST_BYTES", args=[node.value], result=res))
            return res
        if isinstance(node.value, float):
            res = MoltValue(self.next_var(), type_hint="float")
            self.emit(MoltOp(kind="CONST_FLOAT", args=[node.value], result=res))
            return res
        if isinstance(node.value, complex):
            # TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:missing): support complex literals + lowering.
            raise self.compat.unsupported(
                node,
                feature="complex literals",
                tier="bridge",
                impact="high",
            )
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

    def _emit_repr_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="REPR_FROM_OBJ", args=[value], result=res))
        return res

    def _emit_ascii_from_obj(self, value: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="ASCII_FROM_OBJ", args=[value], result=res))
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

    def _emit_string_format_value(self, value: MoltValue, spec: MoltValue) -> MoltValue:
        res = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="STRING_FORMAT", args=[value, spec], result=res))
        return res

    def _emit_string_format(self, value: MoltValue, spec: str) -> MoltValue:
        spec_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[spec], result=spec_val))
        return self._emit_string_format_value(value, spec_val)

    def _split_format_field_name(
        self, field_name: str
    ) -> tuple[int | str, list[tuple[bool, int | str]]] | None:
        if not field_name:
            return None
        idx = 0
        while idx < len(field_name) and field_name[idx] not in ".[":
            idx += 1
        first_text = field_name[:idx]
        if not first_text:
            return None
        if first_text.isdigit():
            first: int | str = int(first_text)
        else:
            first = first_text
        rest_items: list[tuple[bool, int | str]] = []
        while idx < len(field_name):
            ch = field_name[idx]
            if ch == ".":
                idx += 1
                start = idx
                while idx < len(field_name) and field_name[idx] not in ".[":
                    idx += 1
                if idx == start:
                    return None
                rest_items.append((True, field_name[start:idx]))
                continue
            if ch == "[":
                idx += 1
                start = idx
                while idx < len(field_name) and field_name[idx] != "]":
                    idx += 1
                if idx >= len(field_name):
                    return None
                key_text = field_name[start:idx]
                if not key_text:
                    return None
                if key_text.isdigit():
                    key: int | str = int(key_text)
                else:
                    key = key_text
                rest_items.append((False, key))
                idx += 1
                continue
            return None
        return first, rest_items

    def _parse_format_tokens(
        self,
        text: str,
        arg_count: int,
        kw_names: set[str],
        state: FormatParseState,
    ) -> list[FormatToken] | None:
        tokens: list[FormatToken] = []
        try:
            parsed = _py_string.Formatter().parse(text)
        except ValueError:
            return None
        for literal_text, field_name, format_spec, conversion in parsed:
            if literal_text:
                if tokens and isinstance(tokens[-1], FormatLiteral):
                    prior = cast(FormatLiteral, tokens[-1])
                    tokens[-1] = FormatLiteral(prior.text + literal_text)
                else:
                    tokens.append(FormatLiteral(literal_text))
            if field_name is None:
                continue
            if conversion is not None and conversion not in {"r", "s", "a"}:
                return None
            if field_name == "":
                if state.used_manual:
                    return None
                state.used_auto = True
                key: int | str = state.next_auto
                state.next_auto += 1
                rest_items: list[tuple[bool, int | str]] = []
            else:
                if state.used_auto:
                    return None
                state.used_manual = True
                parsed_field = self._split_format_field_name(field_name)
                if parsed_field is None:
                    return None
                key, rest_items = parsed_field
            if isinstance(key, int):
                if key < 0 or key >= arg_count:
                    return None
            else:
                if key not in kw_names:
                    return None
            spec_tokens: list[FormatToken] | None = None
            if format_spec:
                spec_tokens = self._parse_format_tokens(
                    format_spec,
                    arg_count,
                    kw_names,
                    state,
                )
                if spec_tokens is None:
                    return None
            tokens.append(FormatField(key, rest_items, conversion, spec_tokens))
        return tokens

    def _emit_format_tokens(
        self,
        tokens: list[FormatToken],
        args: list[MoltValue],
        kwargs: dict[str, MoltValue],
    ) -> MoltValue:
        parts: list[MoltValue] = []
        for token in tokens:
            if isinstance(token, FormatLiteral):
                parts.append(self._emit_const_value(token.text))
                continue
            if isinstance(token.key, int):
                value = args[token.key]
            else:
                value = kwargs[token.key]
            for is_attr, name in token.rest:
                if is_attr:
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="GETATTR_GENERIC_OBJ",
                            args=[value, name],
                            result=res,
                        )
                    )
                    value = res
                else:
                    key_val = self._emit_const_value(name)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="INDEX", args=[value, key_val], result=res))
                    value = res
            if token.conversion is not None:
                if token.conversion == "r":
                    value = self._emit_repr_from_obj(value)
                elif token.conversion == "s":
                    value = self._emit_str_from_obj(value)
                elif token.conversion == "a":
                    value = self._emit_ascii_from_obj(value)
            if token.format_spec is None:
                spec_val = self._emit_const_value("")
            else:
                spec_val = self._emit_format_tokens(token.format_spec, args, kwargs)
            parts.append(self._emit_string_format_value(value, spec_val))
        return self._emit_string_join(parts)

    def _lower_string_format_call(
        self, node: ast.Call, format_str: str
    ) -> MoltValue | None:
        # TODO(perf, owner:frontend, milestone:TC2, priority:P2, status:planned): cache
        # parsed format tokens for constant format strings to avoid repeated parsing.
        if any(isinstance(arg, ast.Starred) for arg in node.args):
            return None
        kw_names: list[str] = []
        for keyword in node.keywords:
            if keyword.arg is None:
                return None
            kw_names.append(keyword.arg)
        if len(set(kw_names)) != len(kw_names):
            return None
        state = FormatParseState()
        try:
            tokens = self._parse_format_tokens(
                format_str,
                len(node.args),
                set(kw_names),
                state,
            )
        except ValueError as exc:
            err_val = self._emit_exception_new("ValueError", str(exc))
            self.emit(MoltOp(kind="RAISE", args=[err_val], result=MoltValue("none")))
            res = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
            return res
        if tokens is None:
            return None
        args: list[MoltValue] = []
        for arg in node.args:
            value = self.visit(arg)
            if value is None:
                raise NotImplementedError("Unsupported format argument")
            args.append(value)
        kwargs: dict[str, MoltValue] = {}
        for keyword in node.keywords:
            value = self.visit(keyword.value)
            if value is None:
                raise NotImplementedError("Unsupported format argument")
            key = keyword.arg
            if key is None:
                raise NotImplementedError("Unsupported format argument")
            kwargs[key] = value
        return self._emit_format_tokens(tokens, args, kwargs)

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
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Lt):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[left, right], result=res))
            return res
        if isinstance(op, ast.Gt):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="GT", args=[left, right], result=res))
            return res
        if isinstance(op, ast.LtE):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LE", args=[left, right], result=res))
            return res
        if isinstance(op, ast.GtE):
            res = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="GE", args=[left, right], result=res))
            return res
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
                value = self.visit(item.value)
                if item.conversion != -1:
                    if item.conversion == ord("r"):
                        value = self._emit_repr_from_obj(value)
                    elif item.conversion == ord("s"):
                        value = self._emit_str_from_obj(value)
                    elif item.conversion == ord("a"):
                        value = self._emit_ascii_from_obj(value)
                    else:
                        raise NotImplementedError(
                            "Formatted value conversion not supported"
                        )
                if item.format_spec is None:
                    if item.conversion != -1:
                        parts.append(value)
                    else:
                        parts.append(self._emit_string_format(value, ""))
                    continue
                spec_text = self._format_spec_to_str(item.format_spec)
                parts.append(self._emit_string_format(value, spec_text))
                continue
            raise NotImplementedError("Unsupported f-string segment")
        return self._emit_string_join(parts)

    def _build_comprehension_body(
        self,
        generators: list[ast.comprehension],
        inner: list[ast.stmt],
    ) -> list[ast.stmt]:
        body = inner
        for comp in reversed(generators):
            for test in reversed(comp.ifs):
                body = [ast.If(test=test, body=body, orelse=[])]
            if comp.is_async:
                body = [
                    ast.AsyncFor(
                        target=comp.target,
                        iter=comp.iter,
                        body=body,
                        orelse=[],
                    )
                ]
            else:
                body = [
                    ast.For(target=comp.target, iter=comp.iter, body=body, orelse=[])
                ]
        return body

    def _comprehension_requires_async(
        self,
        generators: list[ast.comprehension],
        exprs: list[ast.AST | None],
    ) -> bool:
        if any(comp.is_async for comp in generators):
            return True
        for comp in generators:
            if self._expr_needs_async(comp.iter):
                return True
            for test in comp.ifs:
                if self._expr_needs_async(test):
                    return True
        for expr in exprs:
            if expr is None:
                continue
            if self._expr_needs_async(expr):
                return True
        return False

    def visit_ListComp(self, node: ast.ListComp) -> Any:
        async_needed = self._comprehension_requires_async(node.generators, [node.elt])
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        genexp = ast.GeneratorExp(elt=node.elt, generators=node.generators)
        gen_val = self.visit(genexp)
        if gen_val is None:
            raise NotImplementedError("Unsupported list comprehension")
        if async_needed:
            return self._emit_list_from_aiter(gen_val)
        return self._emit_list_from_iter(gen_val)

    def visit_SetComp(self, node: ast.SetComp) -> Any:
        async_needed = self._comprehension_requires_async(node.generators, [node.elt])
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        genexp = ast.GeneratorExp(elt=node.elt, generators=node.generators)
        gen_val = self.visit(genexp)
        if gen_val is None:
            raise NotImplementedError("Unsupported set comprehension")
        if async_needed:
            return self._emit_set_from_aiter(gen_val)
        return self._emit_set_from_iter(gen_val)

    def visit_DictComp(self, node: ast.DictComp) -> Any:
        async_needed = self._comprehension_requires_async(
            node.generators, [node.key, node.value]
        )
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        pair = ast.Tuple(elts=[node.key, node.value], ctx=ast.Load())
        genexp = ast.GeneratorExp(elt=pair, generators=node.generators)
        gen_val = self.visit(genexp)
        if gen_val is None:
            raise NotImplementedError("Unsupported dict comprehension")
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
        if async_needed:
            res = self._emit_dict_fill_from_aiter(res, gen_val)
        else:
            self._emit_dict_fill_from_iter(res, gen_val)
        return res

    def visit_GeneratorExp(self, node: ast.GeneratorExp) -> Any:
        async_needed = self._comprehension_requires_async(node.generators, [node.elt])
        if async_needed and not self.is_async_context():
            raise SyntaxError(
                "asynchronous comprehension outside of an asynchronous function"
            )
        func_symbol = self._genexpr_symbol()
        poll_func_name = f"{func_symbol}_poll"
        prev_func = self.current_func_name
        free_vars: list[str] = []
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        module_namedexpr_targets: set[str] = set()
        if self.current_func_name == "molt_main":
            module_namedexpr_targets = self._collect_namedexpr_targets_comprehension(
                node
            )
            if module_namedexpr_targets:
                self.module_global_mutations.update(module_namedexpr_targets)
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars_comprehension(node)
            if free_vars:
                self.unbound_check_names.update(free_vars)
                for name in free_vars:
                    self._box_local(name)
                for name in free_vars:
                    hint = self.boxed_local_hints.get(name)
                    if hint is None:
                        value = self.locals.get(name)
                        if value is not None and value.type_hint:
                            hint = value.type_hint
                    free_var_hints[name] = hint or "Any"
                closure_items = [self.boxed_locals[name] for name in free_vars]
                closure_val = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(
                    MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                )
                has_closure = True
        prev_state = self._capture_function_state()
        prev_async_context = self.async_context
        self.start_function(
            poll_func_name,
            params=["self"],
            type_facts_name=func_symbol,
            needs_return_slot=False,
        )
        self.async_context = prev_async_context
        self.global_decls = set(module_namedexpr_targets)
        self.in_generator = True
        if has_closure:
            self.async_closure_offset = GEN_CONTROL_SIZE
            self.async_locals_base = GEN_CONTROL_SIZE + 8
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        else:
            self.async_locals_base = GEN_CONTROL_SIZE
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        yield_stmt = ast.Expr(value=ast.Yield(value=node.elt))
        body = self._build_comprehension_body(node.generators, [yield_stmt])
        self._push_qualname("<genexpr>", True)
        try:
            for stmt in body:
                self.visit(stmt)
        finally:
            self._pop_qualname()
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
                self.emit(MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair))
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
        self._spill_async_temporaries()
        closure_size = self.async_locals_base + len(self.async_locals) * 8
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        res = MoltValue(self.next_var(), type_hint="generator")
        args: list[MoltValue] = []
        if has_closure and closure_val is not None:
            args.append(closure_val)
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_func_name, closure_size] + args,
                result=res,
                metadata={"task_kind": "generator"},
            )
        )
        if async_needed:
            async_res = MoltValue(self.next_var(), type_hint="async_generator")
            self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[res], result=async_res))
            return async_res
        return res

    def visit_List(self, node: ast.List) -> Any:
        elems = self._emit_expr_list(node.elts)
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
        elems = self._emit_expr_list(node.elts)
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

    def visit_Set(self, node: ast.Set) -> Any:
        for elt in node.elts:
            if isinstance(elt, ast.Starred):
                raise NotImplementedError("Set unpacking is not supported")
        elems = self._emit_expr_list(node.elts)
        res = MoltValue(self.next_var(), type_hint="set")
        self.emit(MoltOp(kind="SET_NEW", args=elems, result=res))
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
        items: list[ast.expr] = []
        for key, value in zip(node.keys, node.values):
            if key is None:
                raise NotImplementedError("Dict unpacking is not supported")
            items.append(key)
            items.append(value)
        values = self._emit_expr_list(items)
        res = MoltValue(self.next_var(), type_hint="dict")
        self.emit(MoltOp(kind="DICT_NEW", args=values, result=res))
        if values:
            key_vals = values[::2]
            val_vals = values[1::2]
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
        self.local_class_names.add(node.name)
        prev_class_annotations = self.class_annotation_items
        prev_class_exec_map = self.class_annotation_exec_map
        prev_class_exec_name = self.class_annotation_exec_name
        prev_class_exec_counter = self.class_annotation_exec_counter
        self.class_annotation_items = []
        self.class_annotation_exec_map = None
        self.class_annotation_exec_name = None
        self.class_annotation_exec_counter = 0
        dataclass_opts = None
        other_decorators: list[ast.expr] = []
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
                other_decorators.append(deco)
            if dataclass_opts is not None and other_decorators:
                raise NotImplementedError(
                    "Dataclass decorators cannot be combined with other class decorators"
                )

        decorator_vals: list[MoltValue] = []
        if other_decorators:
            for deco in other_decorators:
                decorator_val = self.visit(deco)
                if decorator_val is None:
                    raise NotImplementedError("Unsupported class decorator")
                decorator_vals.append(decorator_val)

        def base_expr_name(expr: ast.expr) -> str | None:
            if isinstance(expr, ast.Name):
                return expr.id
            if isinstance(expr, ast.Attribute):
                parts: list[str] = []
                current: ast.expr | None = expr
                while isinstance(current, ast.Attribute):
                    parts.append(current.attr)
                    current = current.value
                if isinstance(current, ast.Name):
                    parts.append(current.id)
                    parts.reverse()
                    return ".".join(parts)
            return None

        base_vals: list[MoltValue] = []
        base_names: list[str] = []
        if node.bases:
            if node.keywords:
                raise NotImplementedError("Class keywords are not supported")
            for base_expr in node.bases:
                base_name = base_expr_name(base_expr)
                if base_name is None:
                    raise NotImplementedError("Unsupported base class expression")
                base_val = self.visit(base_expr)
                if base_val is None:
                    raise NotImplementedError("Base class must be defined before use")
                base_vals.append(base_val)
                base_names.append(base_name)
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

        methods: dict[str, MethodInfo] = {}
        class_attrs: dict[str, ast.expr] = {}
        class_attr_values: dict[str, MoltValue] = {}
        class_annotation_items: list[tuple[str, MoltValue]] = []
        if len(base_names) != len(set(base_names)):
            dup = next(name for name in base_names if base_names.count(name) > 1)
            raise NotImplementedError(f"Duplicate base class {dup}")

        dynamic = len(base_names) > 1
        if any(
            name not in self.classes and name not in BUILTIN_TYPE_TAGS
            for name in base_names
        ):
            dynamic = True
        for name in base_names:
            base_info = self.classes.get(name)
            if base_info and base_info.get("dynamic"):
                dynamic = True
        if node.name in self.mutated_classes:
            dynamic = True
        is_static = self.current_func_name == "molt_main"

        base_mros = [self._class_mro_names(name) for name in base_names]
        base_mros.append(list(base_names))
        merged = self._c3_merge(base_mros)
        if merged is None:
            merged = list(base_names)
        mro_names = [node.name] + merged

        if dataclass_opts is not None:
            if any(name != "object" for name in base_names):
                raise NotImplementedError("Dataclass inheritance is not supported")
            field_order: list[str] = []
            field_defaults: dict[str, ast.expr] = {}
            field_hints: dict[str, str] = {}
            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    name = item.target.id
                    field_order.append(name)
                    if self._hints_enabled():
                        hint = self._annotation_to_hint(item.annotation)
                        if hint is not None:
                            field_hints[name] = hint
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
                "field_hints": field_hints,
                "class_attrs": class_attrs,
                "module": self.module_name,
                "bases": base_names,
                "mro": mro_names,
                "dynamic": False,
                "static": is_static,
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
            field_hints: dict[str, str] = {}
            for base_name in mro_names[1:]:
                base_info = self.classes.get(base_name)
                if base_info is None:
                    continue
                for field in base_info.get("field_order", []):
                    if field not in fields:
                        fields[field] = len(field_order) * 8
                        field_order.append(field)
                for field, hint in base_info.get("field_hints", {}).items():
                    if field not in field_hints:
                        field_hints[field] = hint
                for name, expr in base_info.get("defaults", {}).items():
                    if name not in field_defaults:
                        field_defaults[name] = expr

            def add_field(name: str) -> None:
                if name in fields:
                    return
                fields[name] = len(field_order) * 8
                field_order.append(name)

            def add_field_hint(name: str, annotation: ast.AST | None) -> None:
                if not self._hints_enabled() or annotation is None:
                    return
                hint = self._annotation_to_hint(cast(ast.expr, annotation))
                if hint is None or name in field_hints:
                    return
                field_hints[name] = hint

            for item in node.body:
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    add_field(item.target.id)
                    add_field_hint(item.target.id, item.annotation)
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
            if any(
                method.name
                in {
                    "__getattr__",
                    "__getattribute__",
                    "__setattr__",
                    "__delattr__",
                }
                for method in methods_in_body
            ):
                dynamic = True

            if methods_in_body:

                class FieldCollector(ast.NodeVisitor):
                    def __init__(
                        self,
                        add: Callable[[str], None],
                        add_hint: Callable[[str, ast.AST | None], None],
                    ) -> None:
                        self._add = add
                        self._add_hint = add_hint

                    def visit_Assign(self, node: ast.Assign) -> None:
                        for target in node.targets:
                            self._handle_target(target)
                        self.generic_visit(node.value)

                    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
                        self._handle_target(node.target, node.annotation)
                        if node.value is not None:
                            self.generic_visit(node.value)

                    def _handle_target(
                        self, target: ast.AST, annotation: ast.AST | None = None
                    ) -> None:
                        if (
                            isinstance(target, ast.Attribute)
                            and isinstance(target.value, ast.Name)
                            and target.value.id == "self"
                        ):
                            self._add(target.attr)
                            self._add_hint(target.attr, annotation)

                    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
                        return

                    def visit_AsyncFunctionDef(
                        self, node: ast.AsyncFunctionDef
                    ) -> None:
                        return

                    def visit_Lambda(self, node: ast.Lambda) -> None:
                        return

                collector = FieldCollector(add_field, add_field_hint)
                for method in methods_in_body:
                    for stmt in method.body:
                        collector.visit(stmt)

            self.classes[node.name] = ClassInfo(
                fields=fields,
                size=(len(field_order) * 8 + 8) if not dynamic else 8,
                methods=methods,
                field_order=field_order,
                defaults=field_defaults,
                field_hints=field_hints,
                class_attrs=class_attrs,
                module=self.module_name,
                bases=base_names,
                mro=mro_names,
                dynamic=dynamic,
                static=is_static,
            )

        method_count = sum(
            isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
            for item in node.body
        )
        self.classes[node.name]["layout_version"] = self._class_layout_version(
            node.name,
            class_attrs,
            method_count=method_count,
        )

        def compile_generator_method(item: ast.FunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function", "classmethod", "staticmethod", "property", "decorated"
            ] = "function"
            decorator_vals: list[MoltValue] = []
            if item.decorator_list:
                if len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Name
                ):
                    deco = item.decorator_list[0]
                    if deco.id in {"classmethod", "staticmethod", "property"}:
                        descriptor = cast(
                            Literal[
                                "function",
                                "classmethod",
                                "staticmethod",
                                "property",
                                "decorated",
                            ],
                            deco.id,
                        )
                    else:
                        decorator_val = self.visit(deco)
                        if decorator_val is None:
                            raise NotImplementedError("Unsupported method decorator")
                        decorator_vals.append(decorator_val)
                        descriptor = "decorated"
                else:
                    for deco in item.decorator_list:
                        decorator_val = self.visit(deco)
                        if decorator_val is None:
                            raise NotImplementedError("Unsupported method decorator")
                        decorator_vals.append(decorator_val)
                    descriptor = "decorated"
            method_name = item.name
            property_field = None
            if descriptor == "property":
                property_field = self._property_field_from_method(item)
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
            self._record_func_default_specs(method_symbol, item.args)
            poll_symbol = f"{method_symbol}_poll"
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            default_specs = self._default_specs_from_args(item.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            free_vars: list[str] = []
            free_var_hints: dict[str, str] = {}
            closure_val: MoltValue | None = None
            has_closure = False
            if self.current_func_name != "molt_main":
                free_vars = self._collect_free_vars(item)
                if free_vars:
                    self.unbound_check_names.update(free_vars)
                    for name in free_vars:
                        self._box_local(name)
                    for name in free_vars:
                        hint = self.boxed_local_hints.get(name)
                        if hint is None:
                            value = self.locals.get(name)
                            if value is not None and value.type_hint:
                                hint = value.type_hint
                        free_var_hints[name] = hint or "Any"
                    closure_items = [self.boxed_locals[name] for name in free_vars]
                    closure_val = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                    )
                    has_closure = True
            has_return = self._function_contains_return(item)
            func_kind = "GenClosureFunc" if has_closure else "GenFunc"
            func_val = MoltValue(
                self.next_var(), type_hint=f"{func_kind}:{poll_symbol}:0"
            )
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[poll_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[poll_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and self._signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=item.args.defaults,
                kw_default_exprs=item.args.kw_defaults,
                docstring=ast.get_docstring(item),
                is_generator=True,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, item)

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
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            self.in_generator = True
            if has_closure:
                self.async_closure_offset = GEN_CONTROL_SIZE
                self.async_locals_base = GEN_CONTROL_SIZE + 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            else:
                self.async_locals_base = GEN_CONTROL_SIZE
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                if self._hints_enabled():
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
                    if hint is not None:
                        self.async_local_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
                    if isinstance(stmt, (ast.Return, ast.Raise)):
                        break
            finally:
                self._pop_qualname()
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
            self._spill_async_temporaries()
            closure_size = self.async_locals_base + len(self.async_locals) * 8
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param
            func_val.type_hint = f"{func_kind}:{poll_symbol}:{closure_size}"
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )

            method_attr = func_val
            if descriptor == "decorated":
                decorated = func_val
                for decorator_val in reversed(decorator_vals):
                    callargs = MoltValue(self.next_var(), type_hint="callargs")
                    self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                    push_res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(
                        MoltOp(
                            kind="CALLARGS_PUSH_POS",
                            args=[callargs, decorated],
                            result=push_res,
                        )
                    )
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[decorator_val, callargs],
                            result=res,
                        )
                    )
                    decorated = res
                method_attr = decorated
            elif descriptor == "classmethod":
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
                "param_count": len(params),
                "defaults": default_specs,
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                "property_field": property_field,
            }

        def compile_method(item: ast.FunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function", "classmethod", "staticmethod", "property", "decorated"
            ] = "function"
            decorator_vals: list[MoltValue] = []
            if item.decorator_list:
                if len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Name
                ):
                    deco = item.decorator_list[0]
                    if deco.id in {"classmethod", "staticmethod", "property"}:
                        descriptor = cast(
                            Literal[
                                "function",
                                "classmethod",
                                "staticmethod",
                                "property",
                                "decorated",
                            ],
                            deco.id,
                        )
                    else:
                        decorator_val = self.visit(deco)
                        if decorator_val is None:
                            raise NotImplementedError("Unsupported method decorator")
                        decorator_vals.append(decorator_val)
                        descriptor = "decorated"
                else:
                    for deco in item.decorator_list:
                        decorator_val = self.visit(deco)
                        if decorator_val is None:
                            raise NotImplementedError("Unsupported method decorator")
                        decorator_vals.append(decorator_val)
                    descriptor = "decorated"
            method_name = item.name
            property_field = None
            if descriptor == "property":
                property_field = self._property_field_from_method(item)
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
            self._record_func_default_specs(method_symbol, item.args)
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            default_specs = self._default_specs_from_args(item.args)
            free_vars: list[str] = []
            free_var_hints: dict[str, str] = {}
            closure_val: MoltValue | None = None
            has_closure = False
            if self.current_func_name != "molt_main":
                free_vars = self._collect_free_vars(item)
                if free_vars:
                    self.unbound_check_names.update(free_vars)
                    for name in free_vars:
                        self._box_local(name)
                    for name in free_vars:
                        hint = self.boxed_local_hints.get(name)
                        if hint is None:
                            value = self.locals.get(name)
                            if value is not None and value.type_hint:
                                hint = value.type_hint
                        free_var_hints[name] = hint or "Any"
                    closure_items = [self.boxed_locals[name] for name in free_vars]
                    closure_val = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                    )
                    has_closure = True

            func_hint = f"Func:{method_symbol}"
            if has_closure:
                func_hint = f"ClosureFunc:{method_symbol}"
            func_val = MoltValue(self.next_var(), type_hint=func_hint)
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[method_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[method_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and self._signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=item.args.defaults,
                kw_default_exprs=item.args.kw_defaults,
                docstring=ast.get_docstring(item),
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, item)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            prev_class = self.current_class
            prev_first_param = self.current_method_first_param
            self.current_class = node.name
            self.current_method_first_param = params[0] if params else None
            method_params = params
            if has_closure:
                method_params = [_MOLT_CLOSURE_PARAM] + params
            self.start_function(
                method_symbol,
                params=method_params,
                type_facts_name=f"{node.name}.{method_name}",
                needs_return_slot=False,
            )
            if has_closure:
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
                self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                    _MOLT_CLOSURE_PARAM, type_hint="tuple"
                )
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            for idx, arg in enumerate(arg_nodes):
                hint = None
                if idx == 0 and descriptor == "classmethod":
                    hint = node.name
                elif idx == 0 and arg.arg == "self":
                    hint = node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in item.args.args:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            for name in sorted(self.scope_assigned):
                self._box_local(name)
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
            finally:
                self._pop_qualname()
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
            if descriptor == "decorated":
                decorated = func_val
                for decorator_val in reversed(decorator_vals):
                    callargs = MoltValue(self.next_var(), type_hint="callargs")
                    self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                    push_res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(
                        MoltOp(
                            kind="CALLARGS_PUSH_POS",
                            args=[callargs, decorated],
                            result=push_res,
                        )
                    )
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[decorator_val, callargs],
                            result=res,
                        )
                    )
                    decorated = res
                method_attr = decorated
            elif descriptor == "classmethod":
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
                "param_count": len(params),
                "defaults": default_specs,
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                "property_field": property_field,
            }

        def compile_async_method(item: ast.AsyncFunctionDef) -> MethodInfo:
            descriptor: Literal[
                "function", "classmethod", "staticmethod", "property", "decorated"
            ] = "function"
            decorator_vals: list[MoltValue] = []
            if item.decorator_list:
                if len(item.decorator_list) == 1 and isinstance(
                    item.decorator_list[0], ast.Name
                ):
                    deco = item.decorator_list[0]
                    if deco.id in {"classmethod", "staticmethod", "property"}:
                        descriptor = cast(
                            Literal[
                                "function",
                                "classmethod",
                                "staticmethod",
                                "property",
                                "decorated",
                            ],
                            deco.id,
                        )
                    else:
                        decorator_val = self.visit(deco)
                        if decorator_val is None:
                            raise NotImplementedError("Unsupported method decorator")
                        decorator_vals.append(decorator_val)
                        descriptor = "decorated"
                else:
                    for deco in item.decorator_list:
                        decorator_val = self.visit(deco)
                        if decorator_val is None:
                            raise NotImplementedError("Unsupported method decorator")
                        decorator_vals.append(decorator_val)
                    descriptor = "decorated"
            is_async_gen = self._function_contains_yield(item)
            if is_async_gen:
                if self._async_generator_contains_yield_from(item):
                    raise SyntaxError("'yield from' inside async function")
                if self._async_generator_contains_return_value(item):
                    raise SyntaxError("'return' with value in async generator")
                method_name = item.name
                property_field = None
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
                self._record_func_default_specs(wrapper_symbol, item.args)
                poll_symbol = f"{wrapper_symbol}_poll"
                posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                    item.args
                )
                posonly_names = [arg.arg for arg in posonly]
                pos_or_kw_names = [arg.arg for arg in pos_or_kw]
                kwonly_names = [arg.arg for arg in kwonly]
                params = self._function_param_names(item.args)
                arg_nodes: list[ast.arg] = posonly + pos_or_kw
                if item.args.vararg is not None:
                    arg_nodes.append(item.args.vararg)
                arg_nodes.extend(kwonly)
                if item.args.kwarg is not None:
                    arg_nodes.append(item.args.kwarg)
                default_specs = self._default_specs_from_args(item.args)
                free_vars: list[str] = []
                free_var_hints: dict[str, str] = {}
                closure_val: MoltValue | None = None
                has_closure = False
                if self.current_func_name != "molt_main":
                    free_vars = self._collect_free_vars(item)
                    if free_vars:
                        self.unbound_check_names.update(free_vars)
                        for name in free_vars:
                            self._box_local(name)
                        for name in free_vars:
                            hint = self.boxed_local_hints.get(name)
                            if hint is None:
                                value = self.locals.get(name)
                                if value is not None and value.type_hint:
                                    hint = value.type_hint
                            free_var_hints[name] = hint or "Any"
                        closure_items = [self.boxed_locals[name] for name in free_vars]
                        closure_val = MoltValue(self.next_var(), type_hint="tuple")
                        self.emit(
                            MoltOp(
                                kind="TUPLE_NEW", args=closure_items, result=closure_val
                            )
                        )
                        has_closure = True
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
                self.global_decls = self._collect_global_decls(item.body)
                self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
                assigned = self._collect_assigned_names(item.body)
                self.del_targets = self._collect_deleted_names(item.body)
                self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
                self.unbound_check_names = set(self.scope_assigned)
                self.in_generator = True
                if has_closure:
                    self.async_closure_offset = GEN_CONTROL_SIZE
                    self.async_locals_base = GEN_CONTROL_SIZE + 8
                    self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                    self.free_var_hints = free_var_hints
                else:
                    self.async_locals_base = GEN_CONTROL_SIZE
                for i, arg in enumerate(arg_nodes):
                    self.async_locals[arg.arg] = self.async_locals_base + i * 8
                    hint = None
                    if i == 0 and descriptor == "classmethod":
                        hint = node.name
                    elif i == 0 and arg.arg == "self":
                        hint = node.name
                    if self._hints_enabled():
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
                self.emit(
                    MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none"))
                )
                self._init_scope_async_locals(arg_nodes)
                if self.type_hint_policy == "check":
                    for arg in arg_nodes:
                        hint = self.explicit_type_hints.get(arg.arg)
                        if hint is not None:
                            self._emit_guard_type(
                                MoltValue(arg.arg, type_hint=hint), hint
                            )
                self._push_qualname(method_name, True)
                try:
                    for stmt in item.body:
                        self.visit(stmt)
                        if isinstance(stmt, (ast.Return, ast.Raise)):
                            break
                finally:
                    self._pop_qualname()
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
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=[none_val, done], result=pair)
                    )
                    self.emit(MoltOp(kind="ret", args=[pair], result=MoltValue("none")))
                self._spill_async_temporaries()
                closure_size = self.async_locals_base + len(self.async_locals) * 8
                self.resume_function(prev_func)
                self._restore_function_state(prev_state)
                self.current_class = prev_class
                self.current_method_first_param = prev_first_param

                func_hint = f"Func:{wrapper_symbol}"
                if has_closure:
                    func_hint = f"ClosureFunc:{wrapper_symbol}"
                func_val = MoltValue(self.next_var(), type_hint=func_hint)
                if has_closure and closure_val is not None:
                    self.emit(
                        MoltOp(
                            kind="FUNC_NEW_CLOSURE",
                            args=[wrapper_symbol, len(params), closure_val],
                            result=func_val,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="FUNC_NEW",
                            args=[wrapper_symbol, len(params)],
                            result=func_val,
                        )
                    )
                func_spill = None
                if self.in_generator and self._signature_contains_yield(
                    decorators=item.decorator_list,
                    args=item.args,
                    returns=item.returns,
                ):
                    func_spill = self._spill_async_value(
                        func_val, f"__func_meta_{len(self.async_locals)}"
                    )
                self._emit_function_metadata(
                    func_val,
                    name=method_name,
                    qualname=self._qualname_for_def(method_name),
                    trace_lineno=item.lineno,
                    posonly_params=posonly_names,
                    pos_or_kw_params=pos_or_kw_names,
                    kwonly_params=kwonly_names,
                    vararg=vararg,
                    varkw=varkw,
                    default_exprs=item.args.defaults,
                    kw_default_exprs=item.args.kw_defaults,
                    docstring=ast.get_docstring(item),
                    is_async_generator=True,
                )
                if func_spill is not None:
                    func_val = self._reload_async_value(func_spill, func_val.type_hint)
                self._emit_function_annotate(func_val, item)

                prev_func = self.current_func_name
                prev_state = self._capture_function_state()
                wrapper_params = params
                if has_closure:
                    wrapper_params = [_MOLT_CLOSURE_PARAM] + params
                self.start_function(
                    wrapper_symbol,
                    params=wrapper_params,
                    type_facts_name=f"{node.name}.{method_name}",
                )
                if has_closure:
                    self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                        _MOLT_CLOSURE_PARAM, type_hint="tuple"
                    )
                self.global_decls = set()
                self.nonlocal_decls = set()
                self.scope_assigned = set()
                self.del_targets = set()
                for idx, arg in enumerate(arg_nodes):
                    hint = None
                    if idx == 0 and descriptor == "classmethod":
                        hint = node.name
                    elif idx == 0 and arg.arg == "self":
                        hint = node.name
                    if self._hints_enabled():
                        explicit = self.explicit_type_hints.get(arg.arg)
                        if explicit is None:
                            explicit = self._annotation_to_hint(arg.annotation)
                            if explicit is not None:
                                self.explicit_type_hints[arg.arg] = explicit
                        if explicit is not None:
                            hint = explicit
                        elif hint is None:
                            hint = "Any"
                    value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                    if hint is not None:
                        self._apply_hint_to_value(arg.arg, value, hint)
                    self.locals[arg.arg] = value
                if self.type_hint_policy == "check":
                    for arg in arg_nodes:
                        hint = self.explicit_type_hints.get(arg.arg)
                        if hint is not None:
                            self._emit_guard_type(self.locals[arg.arg], hint)
                args = [self.locals[arg.arg] for arg in arg_nodes]
                if has_closure:
                    args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
                gen_val = MoltValue(self.next_var(), type_hint="generator")
                self.emit(
                    MoltOp(
                        kind="ALLOC_TASK",
                        args=[poll_symbol, closure_size] + args,
                        result=gen_val,
                        metadata={"task_kind": "generator"},
                    )
                )
                res = MoltValue(self.next_var(), type_hint="async_generator")
                self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
                self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
                self.resume_function(prev_func)
                self._restore_function_state(prev_state)

                method_attr = func_val
                if descriptor == "decorated":
                    decorated = func_val
                    for decorator_val in reversed(decorator_vals):
                        callargs = MoltValue(self.next_var(), type_hint="callargs")
                        self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                        push_res = MoltValue(self.next_var(), type_hint="None")
                        self.emit(
                            MoltOp(
                                kind="CALLARGS_PUSH_POS",
                                args=[callargs, decorated],
                                result=push_res,
                            )
                        )
                        res = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[decorator_val, callargs],
                                result=res,
                            )
                        )
                        decorated = res
                    method_attr = decorated
                elif descriptor == "classmethod":
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
                    "param_count": len(params),
                    "defaults": default_specs,
                    "has_vararg": vararg is not None,
                    "has_varkw": varkw is not None,
                    "has_closure": has_closure,
                    "property_field": property_field,
                }
            method_name = item.name
            property_field = None
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
            self._record_func_default_specs(wrapper_symbol, item.args)
            poll_symbol = f"{wrapper_symbol}_poll"
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                item.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(item.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if item.args.vararg is not None:
                arg_nodes.append(item.args.vararg)
            arg_nodes.extend(kwonly)
            if item.args.kwarg is not None:
                arg_nodes.append(item.args.kwarg)
            default_specs = self._default_specs_from_args(item.args)
            free_vars: list[str] = []
            free_var_hints: dict[str, str] = {}
            closure_val: MoltValue | None = None
            has_closure = False
            if self.current_func_name != "molt_main":
                free_vars = self._collect_free_vars(item)
                if free_vars:
                    self.unbound_check_names.update(free_vars)
                    for name in free_vars:
                        self._box_local(name)
                    for name in free_vars:
                        hint = self.boxed_local_hints.get(name)
                        if hint is None:
                            value = self.locals.get(name)
                            if value is not None and value.type_hint:
                                hint = value.type_hint
                        free_var_hints[name] = hint or "Any"
                    closure_items = [self.boxed_locals[name] for name in free_vars]
                    closure_val = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                    )
                    has_closure = True
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
            self.global_decls = self._collect_global_decls(item.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(item.body)
            assigned = self._collect_assigned_names(item.body)
            self.del_targets = self._collect_deleted_names(item.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            if has_closure:
                self.async_closure_offset = 0
                self.async_locals_base = 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                hint = None
                if i == 0 and descriptor == "classmethod":
                    hint = node.name
                elif i == 0 and arg.arg == "self":
                    hint = node.name
                if self._hints_enabled():
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
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._push_qualname(method_name, True)
            try:
                for stmt in item.body:
                    self.visit(stmt)
            finally:
                self._pop_qualname()
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
            self._spill_async_temporaries()
            closure_size = self.async_locals_base + len(self.async_locals) * 8
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            self.current_class = prev_class
            self.current_method_first_param = prev_first_param

            func_hint = f"Func:{wrapper_symbol}"
            if has_closure:
                func_hint = f"ClosureFunc:{wrapper_symbol}"
            func_val = MoltValue(self.next_var(), type_hint=func_hint)
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[wrapper_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[wrapper_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and self._signature_contains_yield(
                decorators=item.decorator_list,
                args=item.args,
                returns=item.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            self._emit_function_metadata(
                func_val,
                name=method_name,
                qualname=self._qualname_for_def(method_name),
                trace_lineno=item.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=item.args.defaults,
                kw_default_exprs=item.args.kw_defaults,
                docstring=ast.get_docstring(item),
                is_coroutine=True,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, item)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            wrapper_params = params
            if has_closure:
                wrapper_params = [_MOLT_CLOSURE_PARAM] + params
            self.start_function(
                wrapper_symbol,
                params=wrapper_params,
                type_facts_name=f"{node.name}.{method_name}",
            )
            if has_closure:
                self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                    _MOLT_CLOSURE_PARAM, type_hint="tuple"
                )
            self.global_decls = set()
            self.nonlocal_decls = set()
            self.scope_assigned = set()
            self.del_targets = set()
            for idx, arg in enumerate(arg_nodes):
                hint = None
                if idx == 0 and descriptor == "classmethod":
                    hint = node.name
                elif idx == 0 and arg.arg == "self":
                    hint = node.name
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            args = [self.locals[arg.arg] for arg in arg_nodes]
            if has_closure:
                args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
            res = MoltValue(self.next_var(), type_hint="Future")
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[poll_symbol, closure_size] + args,
                    result=res,
                    metadata={"task_kind": "future"},
                )
            )
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)

            method_attr = func_val
            if descriptor == "decorated":
                decorated = func_val
                for decorator_val in reversed(decorator_vals):
                    callargs = MoltValue(self.next_var(), type_hint="callargs")
                    self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                    push_res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(
                        MoltOp(
                            kind="CALLARGS_PUSH_POS",
                            args=[callargs, decorated],
                            result=push_res,
                        )
                    )
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[decorator_val, callargs],
                            result=res,
                        )
                    )
                    decorated = res
                method_attr = decorated
            elif descriptor == "classmethod":
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
                "param_count": len(params),
                "defaults": default_specs,
                "has_vararg": vararg is not None,
                "has_varkw": varkw is not None,
                "has_closure": has_closure,
                "property_field": property_field,
            }

        self._push_qualname(node.name, False)
        try:
            for item in node.body:
                if isinstance(item, ast.FunctionDef):
                    if self._function_contains_yield(item):
                        methods[item.name] = compile_generator_method(item)
                    else:
                        methods[item.name] = compile_method(item)
                elif isinstance(item, ast.AsyncFunctionDef):
                    methods[item.name] = compile_async_method(item)
        finally:
            self._pop_qualname()

        layout_version = self._class_layout_version(
            node.name, class_attrs, methods=methods
        )
        prior_layout = self.classes[node.name].get("layout_version")
        if prior_layout is not None and prior_layout != layout_version:
            raise RuntimeError(
                "Class layout version changed after method compilation for "
                f"{node.name}: pre={prior_layout} post={layout_version}"
            )
        self.classes[node.name]["layout_version"] = layout_version

        class_scope: dict[str, MoltValue] = {
            name: info["attr"] for name, info in methods.items()
        }
        saved_locals = self.locals
        self.locals = dict(class_scope)
        try:
            for item in node.body:
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    method_info = methods.get(item.name)
                    if method_info is not None:
                        class_scope[item.name] = method_info["attr"]
                        self.locals[item.name] = method_info["attr"]
                    continue
                if isinstance(item, ast.Expr):
                    if isinstance(item.value, ast.Constant) and isinstance(
                        item.value.value, str
                    ):
                        continue
                    if self.visit(item.value) is None:
                        raise NotImplementedError("Unsupported class body expression")
                    continue
                if isinstance(item, ast.Assign):
                    val = self.visit(item.value)
                    if val is None:
                        raise NotImplementedError("Unsupported class body assignment")
                    for target in item.targets:
                        if isinstance(target, ast.Name):
                            class_attr_values[target.id] = val
                            self.locals[target.id] = val
                    continue
                if isinstance(item, ast.AnnAssign) and isinstance(
                    item.target, ast.Name
                ):
                    if self.future_annotations:
                        ann_val = self._emit_annotation_value(
                            item.annotation, stringize=True
                        )
                        class_annotation_items.append((item.target.id, ann_val))
                    else:
                        exec_map = self._ensure_class_annotation_exec_map(node.name)
                        exec_id = self._annotation_exec_id(is_module=False)
                        self._emit_annotation_exec_mark(exec_map, exec_id)
                        self.class_annotation_items.append(
                            (item.target.id, item.annotation, exec_id)
                        )
                    if item.value is None:
                        continue
                    val = self.visit(item.value)
                    if val is None:
                        raise NotImplementedError("Unsupported class body assignment")
                    class_attr_values[item.target.id] = val
                    self.locals[item.target.id] = val
        finally:
            self.locals = saved_locals

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
        if self.current_func_name == "molt_main":
            self.globals[node.name] = class_val
            self._emit_module_attr_set(node.name, class_val)
        else:
            self._store_local_value(node.name, class_val)

        qualname = self._qualname_for_def(node.name)
        qualname_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[qualname], result=qualname_val))
        module_name = (
            "__main__"
            if self.entry_module and self.module_name == self.entry_module
            else self.module_name
        )
        module_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=[module_name], result=module_val))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[class_val, "__name__", name_val],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[class_val, "__qualname__", qualname_val],
                result=MoltValue("none"),
            )
        )
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[class_val, "__module__", module_val],
                result=MoltValue("none"),
            )
        )

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

        size_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[class_info["size"]], result=size_val))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[class_val, "__molt_layout_size__", size_val],
                result=MoltValue("none"),
            )
        )

        for attr_name, val in class_attr_values.items():
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[class_val, attr_name, val],
                    result=MoltValue("none"),
                )
            )

        if (
            self.future_annotations
            and class_annotation_items
            and "__annotations__" not in class_attr_values
        ):
            ann_items: list[MoltValue] = []
            for name, val in class_annotation_items:
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[name], result=key_val))
                ann_items.extend([key_val, val])
            ann_dict = MoltValue(self.next_var(), type_hint="dict")
            self.emit(MoltOp(kind="DICT_NEW", args=ann_items, result=ann_dict))
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[class_val, "__annotations__", ann_dict],
                    result=MoltValue("none"),
                )
            )
        if (
            not self.future_annotations
            and self.class_annotation_items
            and "__annotations__" not in class_attr_values
        ):
            class_scope_names = set(class_attr_values) | set(methods)
            rewritten_items: list[tuple[str, ast.expr, int]] = []
            for name, expr, exec_id in self.class_annotation_items:
                rewritten = self._rewrite_class_annotation_expr(
                    expr, node.name, class_scope_names
                )
                rewritten_items.append((name, rewritten, exec_id))
            annotate_val = self._emit_annotate_function_obj(
                items=rewritten_items,
                exec_map_name=self.class_annotation_exec_name,
                stringize=False,
                module_override=module_name,
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[class_val, "__annotate__", annotate_val],
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
        layout_version = self.classes[node.name].get("layout_version", 0)
        layout_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[layout_version], result=layout_val))
        self.emit(
            MoltOp(
                kind="CLASS_SET_LAYOUT_VERSION",
                args=[class_val, layout_val],
                result=MoltValue("none"),
            )
        )

        if decorator_vals:
            decorated = class_val
            for decorator_val in reversed(decorator_vals):
                callargs = MoltValue(self.next_var(), type_hint="callargs")
                self.emit(MoltOp(kind="CALLARGS_NEW", args=[], result=callargs))
                push_res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="CALLARGS_PUSH_POS",
                        args=[callargs, decorated],
                        result=push_res,
                    )
                )
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="CALL_BIND", args=[decorator_val, callargs], result=res)
                )
                decorated = res
            class_val = decorated
            if self.current_func_name == "molt_main":
                self.globals[node.name] = class_val
                self._emit_module_attr_set(node.name, class_val)
            else:
                self._store_local_value(node.name, class_val)

        self.class_annotation_items = prev_class_annotations
        self.class_annotation_exec_map = prev_class_exec_map
        self.class_annotation_exec_name = prev_class_exec_name
        self.class_annotation_exec_counter = prev_class_exec_counter
        return None

    def _emit_dynamic_call(
        self, node: ast.Call, callee: MoltValue, needs_bind: bool
    ) -> MoltValue:
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
        if needs_bind:
            callargs = self._emit_call_args_builder(node)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
            return res
        if callee.type_hint.startswith("BoundMethod:"):
            args = self._emit_call_args(node.args)
            res = MoltValue(self.next_var(), type_hint=res_hint)
            self.emit(MoltOp(kind="CALL_METHOD", args=[callee] + args, result=res))
            return res
        if callee.type_hint.startswith("Func:"):
            func_symbol = callee.type_hint.split(":", 1)[1]
            args, _ = self._emit_direct_call_args_for_symbol(
                func_symbol, node, func_obj=callee
            )
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
            return res
        callargs = self._emit_call_args_builder(node)
        res = MoltValue(self.next_var(), type_hint=res_hint)
        self.emit(MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res))
        return res

    def visit_Call(self, node: ast.Call) -> Any:
        needs_bind = self._call_needs_bind(node)
        if isinstance(node.func, ast.Attribute):
            if (
                node.func.attr == "format"
                and isinstance(node.func.value, ast.Constant)
                and isinstance(node.func.value.value, str)
            ):
                lowered = self._lower_string_format_call(node, node.func.value.value)
                if lowered is not None:
                    return lowered
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
                and node.func.value.id == "math"
                and node.func.attr == "trunc"
            ):
                if len(node.args) != 1:
                    raise NotImplementedError("math.trunc expects 1 argument")
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported math.trunc input")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="TRUNC", args=[value], result=res))
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
                if node.func.attr == "sleep":
                    return self._emit_asyncio_sleep(node.args, node.keywords)

            receiver = self.visit(node.func.value)
            if receiver is None:
                receiver = MoltValue("unknown_obj", type_hint="Unknown")
            method = node.func.attr
            if method == "sort" and receiver.type_hint == "list":
                needs_bind = True
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
                candidate = node.func.value.id
                candidate_info = self.classes.get(candidate)
                if candidate_info is not None:
                    class_name = candidate
                    class_info = candidate_info
            lookup_class = class_name
            if lookup_class is None and receiver.type_hint in self.classes:
                lookup_class = receiver.type_hint
            method_info = None
            method_class = None
            if lookup_class:
                method_info, method_class = self._resolve_method_info(
                    lookup_class, method
                )
            if method_info and (
                needs_bind
                or method_info.get("descriptor") == "decorated"
                or method_info.get("has_vararg", False)
                or method_info.get("has_varkw", False)
                or method_info.get("has_closure", False)
            ):
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                callargs = self._emit_call_args_builder(node)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND",
                        args=[callee, callargs],
                        result=res,
                    )
                )
                return res
            if method_info and not needs_bind:
                if class_name is None and receiver.type_hint not in self.classes:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
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
                    param_count = method_info.get("param_count")
                    defaults = method_info.get("defaults", [])
                    has_vararg = method_info.get("has_vararg", False)
                    has_varkw = method_info.get("has_varkw", False)
                    if param_count is not None:
                        fixed_param_count = param_count
                        if has_vararg:
                            fixed_param_count -= 1
                        if has_varkw:
                            fixed_param_count -= 1
                        func_obj = None
                        missing = fixed_param_count - len(args)
                        if missing > 0 and any(
                            not spec.get("const", False) for spec in defaults[-missing:]
                        ):
                            class_ref = None
                            if lookup_class:
                                class_info = self.classes.get(lookup_class)
                                if class_info:
                                    class_ref = self._emit_module_attr_get_on(
                                        class_info["module"], lookup_class
                                    )
                            if class_ref is not None:
                                class_attr = self._emit_class_method_func(
                                    class_ref, method
                                )
                                if descriptor == "classmethod":
                                    func_obj = self._emit_bound_method_func(class_attr)
                                else:
                                    func_obj = class_attr
                            else:
                                callee = self.visit(node.func)
                                if callee is not None:
                                    if descriptor == "classmethod":
                                        func_obj = self._emit_bound_method_func(callee)
                                    elif descriptor == "function":
                                        if isinstance(
                                            callee.type_hint, str
                                        ) and callee.type_hint.startswith(
                                            "BoundMethod:"
                                        ):
                                            func_obj = self._emit_bound_method_func(
                                                callee
                                            )
                                        else:
                                            func_obj = callee
                                    else:
                                        func_obj = callee
                        args = self._apply_default_specs(
                            fixed_param_count,
                            defaults,
                            args,
                            node,
                            call_name=f"{lookup_class}.{method}",
                            func_obj=func_obj,
                            implicit_self=False,
                        )
                        if has_vararg:
                            if len(args) > fixed_param_count:
                                extra = args[fixed_param_count:]
                                tuple_val = MoltValue(
                                    self.next_var(), type_hint="tuple"
                                )
                                self.emit(
                                    MoltOp(
                                        kind="TUPLE_NEW",
                                        args=extra,
                                        result=tuple_val,
                                    )
                                )
                                args = args[:fixed_param_count] + [tuple_val]
                            elif len(args) == fixed_param_count:
                                empty_tuple = MoltValue(
                                    self.next_var(), type_hint="tuple"
                                )
                                self.emit(
                                    MoltOp(
                                        kind="TUPLE_NEW",
                                        args=[],
                                        result=empty_tuple,
                                    )
                                )
                                args = args + [empty_tuple]
                        if has_varkw:
                            empty_kwargs = MoltValue(self.next_var(), type_hint="dict")
                            self.emit(
                                MoltOp(
                                    kind="DICT_NEW",
                                    args=[],
                                    result=empty_kwargs,
                                )
                            )
                            args = args + [empty_kwargs]
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
            if method == "add" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.add expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_ADD", args=[receiver, arg], result=res))
                return res
            if method == "discard" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.discard expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_DISCARD", args=[receiver, arg], result=res))
                return res
            if method == "remove" and receiver.type_hint == "set":
                if len(node.args) != 1:
                    raise NotImplementedError("set.remove expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="SET_REMOVE", args=[receiver, arg], result=res))
                return res
            if method in {
                "union",
                "intersection",
                "difference",
                "symmetric_difference",
            } and receiver.type_hint in {"set", "frozenset"}:
                if method == "symmetric_difference":
                    if len(node.args) != 1:
                        raise NotImplementedError(
                            "set.symmetric_difference expects 1 argument"
                        )
                    other = self.visit(node.args[0])
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if other.type_hint not in {"set", "frozenset"}:
                        other = self._emit_set_from_iter(other)
                    op_kind = "BIT_XOR"
                    res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                    self.emit(MoltOp(kind=op_kind, args=[receiver, other], result=res))
                    return res
                if len(node.args) == 0:
                    if receiver.type_hint == "frozenset":
                        return self._emit_frozenset_from_iter(receiver)
                    return self._emit_set_from_iter(receiver)
                if method == "union":
                    res = self._emit_set_from_iter(receiver)
                    for arg in node.args:
                        other = self.visit(arg)
                        if other is None:
                            raise NotImplementedError("Unsupported set operation input")
                        if other.type_hint in {"set", "frozenset"}:
                            self.emit(
                                MoltOp(
                                    kind="SET_UPDATE",
                                    args=[res, other],
                                    result=MoltValue("none"),
                                )
                            )
                        else:
                            self._emit_set_update_from_iter(res, other)
                    if receiver.type_hint == "frozenset":
                        return self._emit_frozenset_from_iter(res)
                    return res
                res = receiver
                for arg in node.args:
                    other = self.visit(arg)
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if other.type_hint not in {"set", "frozenset"}:
                        other = self._emit_set_from_iter(other)
                    op_kind = {
                        "intersection": "BIT_AND",
                        "difference": "SUB",
                    }[method]
                    next_res = MoltValue(self.next_var(), type_hint=receiver.type_hint)
                    self.emit(MoltOp(kind=op_kind, args=[res, other], result=next_res))
                    res = next_res
                return res
            if (
                method
                in {
                    "update",
                    "intersection_update",
                    "difference_update",
                    "symmetric_difference_update",
                }
                and receiver.type_hint == "set"
            ):
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                if method == "symmetric_difference_update":
                    if len(node.args) != 1:
                        raise NotImplementedError(
                            "set.symmetric_difference_update expects 1 argument"
                        )
                if len(node.args) == 0:
                    res = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                    return res
                res = MoltValue(self.next_var(), type_hint="None")
                op_kind = {
                    "update": "SET_UPDATE",
                    "intersection_update": "SET_INTERSECTION_UPDATE",
                    "difference_update": "SET_DIFFERENCE_UPDATE",
                    "symmetric_difference_update": "SET_SYMDIFF_UPDATE",
                }[method]
                for arg in node.args:
                    other = self.visit(arg)
                    if other is None:
                        raise NotImplementedError("Unsupported set operation input")
                    if recv_slot is not None:
                        receiver = self._reload_async_value(
                            recv_slot, receiver.type_hint
                        )
                    if other.type_hint in {"set", "frozenset"} or method != "update":
                        if other.type_hint not in {"set", "frozenset"}:
                            other = self._emit_set_from_iter(other)
                        self.emit(
                            MoltOp(kind=op_kind, args=[receiver, other], result=res)
                        )
                    else:
                        self._emit_set_update_from_iter(receiver, other)
                return res
            if method == "append" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.append expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                arg = self.visit(node.args[0])
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_APPEND", args=[receiver, arg], result=res))
                return res
            if method == "extend" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.extend expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                other = self.visit(node.args[0])
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_EXTEND", args=[receiver, other], result=res)
                )
                return res
            if method == "insert" and receiver.type_hint == "list":
                if len(node.args) != 2:
                    raise NotImplementedError("list.insert expects 2 arguments")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                idx = self.visit(node.args[0])
                val = self.visit(node.args[1])
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="LIST_INSERT", args=[receiver, idx, val], result=res)
                )
                return res
            if method == "remove" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.remove expects 1 argument")
                receiver, recv_slot = self._maybe_spill_receiver(receiver, node.args)
                val = self.visit(node.args[0])
                if recv_slot is not None:
                    receiver = self._reload_async_value(recv_slot, receiver.type_hint)
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REMOVE", args=[receiver, val], result=res))
                return res
            if method == "clear" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.clear expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.copy expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="LIST_COPY", args=[receiver], result=res))
                return res
            if method == "reverse" and receiver.type_hint == "list":
                if node.args or node.keywords:
                    raise NotImplementedError("list.reverse expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="LIST_REVERSE", args=[receiver], result=res))
                return res
            if method == "count" and receiver.type_hint == "list":
                if len(node.args) != 1:
                    raise NotImplementedError("list.count expects 1 argument")
                val = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LIST_COUNT", args=[receiver, val], result=res))
                return res
            if method == "index" and receiver.type_hint == "list":
                # TODO(type-coverage, owner:frontend, milestone:TC1, priority:P2, status:partial):
                # accept keyword args for start/end (list.index(x, start=..., end=...)).
                if node.keywords or len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("list.index expects 1 to 3 arguments")
                val = self.visit(node.args[0])
                if len(node.args) == 1:
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(kind="LIST_INDEX", args=[receiver, val], result=res)
                    )
                    return res
                start = self.visit(node.args[1])
                if len(node.args) == 2:
                    stop = MoltValue(self.next_var(), type_hint="Unknown")
                    self.emit(MoltOp(kind="MISSING", args=[], result=stop))
                else:
                    stop = self.visit(node.args[2])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="LIST_INDEX_RANGE",
                        args=[receiver, val, start, stop],
                        result=res,
                    )
                )
                return res
            if method == "pop" and receiver.type_hint == "dict":
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
            if method == "pop" and receiver.type_hint == "set":
                if node.args:
                    raise NotImplementedError("set.pop expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="SET_POP", args=[receiver], result=res))
                return res
            if method == "pop" and receiver.type_hint == "list":
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
            if method == "get" and receiver.type_hint == "dict":
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
            if method == "setdefault" and receiver.type_hint == "dict":
                if node.keywords or len(node.args) not in (1, 2):
                    raise NotImplementedError(
                        "dict.setdefault expects 1 or 2 arguments"
                    )
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
                    MoltOp(
                        kind="DICT_SETDEFAULT",
                        args=[receiver, key, default],
                        result=res,
                    )
                )
                return res
            if method == "update" and receiver.type_hint == "dict":
                if len(node.args) > 1:
                    msg = f"update expected at most 1 argument, got {len(node.args)}"
                    return self._emit_type_error_value(msg, "None")
                res = MoltValue(self.next_var(), type_hint="None")
                if node.args:
                    other = self.visit(node.args[0])
                    if other is None:
                        raise NotImplementedError("Unsupported dict.update input")
                    self.emit(
                        MoltOp(
                            kind="DICT_UPDATE",
                            args=[receiver, other],
                            result=res,
                        )
                    )
                for kw in node.keywords:
                    if kw.arg is None:
                        mapping = self.visit(kw.value)
                        if mapping is None:
                            raise NotImplementedError(
                                "Unsupported dict.update ** input"
                            )
                        self.emit(
                            MoltOp(
                                kind="DICT_UPDATE_KWSTAR",
                                args=[receiver, mapping],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        key = MoltValue(self.next_var(), type_hint="str")
                        self.emit(MoltOp(kind="CONST_STR", args=[kw.arg], result=key))
                        val = self.visit(kw.value)
                        if val is None:
                            raise NotImplementedError(
                                "Unsupported dict.update kw value"
                            )
                        self.emit(
                            MoltOp(
                                kind="STORE_INDEX",
                                args=[receiver, key, val],
                                result=MoltValue("none"),
                            )
                        )
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=res))
                return res
            if method == "clear" and receiver.type_hint == "dict":
                if node.args or node.keywords:
                    raise NotImplementedError("dict.clear expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="DICT_CLEAR", args=[receiver], result=res))
                return res
            if method == "copy" and receiver.type_hint == "dict":
                if node.args or node.keywords:
                    raise NotImplementedError("dict.copy expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="DICT_COPY", args=[receiver], result=res))
                return res
            if method == "popitem" and receiver.type_hint == "dict":
                if node.args or node.keywords:
                    raise NotImplementedError("dict.popitem expects 0 arguments")
                res = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="DICT_POPITEM", args=[receiver], result=res))
                return res
            if method == "keys" and receiver.type_hint == "dict":
                res = MoltValue(self.next_var(), type_hint="dict_keys_view")
                self.emit(MoltOp(kind="DICT_KEYS", args=[receiver], result=res))
                return res
            if method == "values" and receiver.type_hint == "dict":
                res = MoltValue(self.next_var(), type_hint="dict_values_view")
                self.emit(MoltOp(kind="DICT_VALUES", args=[receiver], result=res))
                return res
            if method == "items" and receiver.type_hint == "dict":
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
            # TODO(stdlib-compat, owner:frontend, milestone:SL1, priority:P2, status:planned):
            # support file.flush via FILE_FLUSH lowering once the op is defined.
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
                # TODO(type-coverage, owner:frontend, milestone:TC1, priority:P2, status:planned):
                # support range-style count keyword args (start/end) once keyword binding
                # is implemented for count.
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("count expects 1-3 arguments")
                needle = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                if receiver.type_hint == "str":
                    res = MoltValue(self.next_var(), type_hint="int")
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_COUNT", args=[receiver, needle], result=res
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if start is None:
                        raise NotImplementedError("Unsupported count start argument")
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        if end is None:
                            raise NotImplementedError("Unsupported count end argument")
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_COUNT_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint in {"bytes", "bytearray"}:
                    res = MoltValue(self.next_var(), type_hint="int")
                    if len(node.args) == 1:
                        op_kind = (
                            "BYTES_COUNT"
                            if receiver.type_hint == "bytes"
                            else "BYTEARRAY_COUNT"
                        )
                        self.emit(
                            MoltOp(kind=op_kind, args=[receiver, needle], result=res)
                        )
                        return res
                    start = self.visit(node.args[1])
                    if start is None:
                        raise NotImplementedError("Unsupported count start argument")
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        if end is None:
                            raise NotImplementedError("Unsupported count end argument")
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    op_kind = (
                        "BYTES_COUNT_SLICE"
                        if receiver.type_hint == "bytes"
                        else "BYTEARRAY_COUNT_SLICE"
                    )
                    self.emit(
                        MoltOp(
                            kind=op_kind,
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            if method == "startswith":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("startswith expects 1-3 arguments")
                needle = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_STARTSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_STARTSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            if method == "endswith":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("endswith expects 1-3 arguments")
                needle = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                res = MoltValue(self.next_var(), type_hint="bool")
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_ENDSWITH",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_ENDSWITH_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
            if method == "join":
                if len(node.args) != 1:
                    obj_name = None
                    exact_class = None
                    if isinstance(node.func.value, ast.Name):
                        obj_name = node.func.value.id
                        exact_class = self.exact_locals.get(obj_name)
                    callee = self._emit_attribute_load(
                        node.func, receiver, obj_name, exact_class
                    )
                    return self._emit_dynamic_call(node, callee, True)
                items = self.visit(node.args[0])
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_JOIN", args=[receiver, items], result=res)
                    )
                    return res
            if method == "split":
                if len(node.args) > 2:
                    raise NotImplementedError("split expects 0-2 arguments")
                if node.args:
                    needle = self.visit(node.args[0])
                else:
                    needle = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=needle))
                if len(node.args) == 2:
                    maxsplit = self.visit(node.args[1])
                if receiver.type_hint in {"Any", "Unknown"}:
                    if needle.type_hint == "str":
                        receiver.type_hint = "str"
                    elif needle.type_hint == "bytearray":
                        receiver.type_hint = "bytearray"
                    elif needle.type_hint == "bytes":
                        receiver.type_hint = "bytes"
                res = MoltValue(self.next_var(), type_hint="list")
                if receiver.type_hint == "str":
                    if len(node.args) == 2:
                        self.emit(
                            MoltOp(
                                kind="STRING_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="STRING_SPLIT", args=[receiver, needle], result=res
                            )
                        )
                    return res
                if receiver.type_hint == "bytes":
                    if len(node.args) == 2:
                        self.emit(
                            MoltOp(
                                kind="BYTES_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="BYTES_SPLIT", args=[receiver, needle], result=res
                            )
                        )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 2:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_SPLIT_MAX",
                                args=[receiver, needle, maxsplit],
                                result=res,
                            )
                        )
                    else:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_SPLIT",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                    return res
            if method == "lower":
                if node.args:
                    raise NotImplementedError("lower expects 0 arguments")
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(MoltOp(kind="STRING_LOWER", args=[receiver], result=res))
                    return res
            if method == "upper":
                if node.args:
                    raise NotImplementedError("upper expects 0 arguments")
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(MoltOp(kind="STRING_UPPER", args=[receiver], result=res))
                    return res
            if method == "capitalize":
                if node.args:
                    raise NotImplementedError("capitalize expects 0 arguments")
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_CAPITALIZE", args=[receiver], result=res)
                    )
                    return res
            if method == "strip":
                if len(node.args) > 1:
                    raise NotImplementedError("strip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_STRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "lstrip":
                if len(node.args) > 1:
                    raise NotImplementedError("lstrip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_LSTRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "rstrip":
                if len(node.args) > 1:
                    raise NotImplementedError("rstrip expects 0 or 1 arguments")
                if node.args:
                    chars = self.visit(node.args[0])
                else:
                    chars = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=chars))
                if receiver.type_hint in {"Any", "Unknown"}:
                    receiver.type_hint = "str"
                res = MoltValue(self.next_var(), type_hint="str")
                if receiver.type_hint == "str":
                    self.emit(
                        MoltOp(kind="STRING_RSTRIP", args=[receiver, chars], result=res)
                    )
                    return res
            if method == "replace":
                if len(node.args) not in (2, 3):
                    raise NotImplementedError("replace expects 2 or 3 arguments")
                old = self.visit(node.args[0])
                new = self.visit(node.args[1])
                if len(node.args) == 3:
                    count = self.visit(node.args[2])
                else:
                    count = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[-1], result=count))
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
                            args=[receiver, old, new, count],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytes":
                    self.emit(
                        MoltOp(
                            kind="BYTES_REPLACE",
                            args=[receiver, old, new, count],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_REPLACE",
                            args=[receiver, old, new, count],
                            result=res,
                        )
                    )
                    return res
            if method == "find":
                if len(node.args) not in (1, 2, 3):
                    raise NotImplementedError("find expects 1-3 arguments")
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
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTES_FIND", args=[receiver, needle], result=res
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTES_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "bytearray":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="BYTEARRAY_FIND",
                                args=[receiver, needle],
                                result=res,
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res
                if receiver.type_hint == "str":
                    if len(node.args) == 1:
                        self.emit(
                            MoltOp(
                                kind="STRING_FIND", args=[receiver, needle], result=res
                            )
                        )
                        return res
                    start = self.visit(node.args[1])
                    if len(node.args) == 3:
                        end = self.visit(node.args[2])
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[True], result=has_end)
                        )
                    else:
                        end = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=end))
                        has_end = MoltValue(self.next_var(), type_hint="bool")
                        self.emit(
                            MoltOp(kind="CONST_BOOL", args=[False], result=has_end)
                        )
                    has_start = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_start))
                    self.emit(
                        MoltOp(
                            kind="STRING_FIND_SLICE",
                            args=[receiver, needle, start, end, has_start, has_end],
                            result=res,
                        )
                    )
                    return res

        if isinstance(node.func, ast.Attribute):
            module_name = None
            if isinstance(node.func.value, ast.Name):
                module_name = self.imported_modules.get(node.func.value.id)
            if module_name:
                func_id = node.func.attr
                normalized = self._normalize_allowlist_module(module_name)
                allowlist_key = normalized or module_name
                if func_id == "field" and allowlist_key == "dataclasses":
                    return self._emit_dataclasses_field_call(allowlist_key, node)
                if func_id == "open" and allowlist_key == "builtins":
                    return self._emit_open_call(node)
                enforce_allowlist = (
                    allowlist_key in MOLT_DIRECT_CALLS
                    or allowlist_key in self.stdlib_allowlist
                )
                if (
                    allowlist_key in MOLT_DIRECT_CALLS
                    and func_id in MOLT_DIRECT_CALLS[allowlist_key]
                ):
                    force_bind = func_id in MOLT_DIRECT_CALL_BIND_ALWAYS.get(
                        allowlist_key, set()
                    )
                    if func_id[:1].isupper():
                        force_bind = True
                    if needs_bind or force_bind:
                        callee = self.visit(node.func)
                        if callee is None:
                            raise NotImplementedError("Unsupported call target")
                        res = MoltValue(self.next_var(), type_hint="Any")
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[callee, callargs],
                                result=res,
                            )
                        )
                        return res
                    args = self._emit_direct_call_args(allowlist_key, func_id, node)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    target_name = (
                        f"{self._sanitize_module_name(allowlist_key)}__{func_id}"
                    )
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                    return res
                if allowlist_key in self.stdlib_allowlist:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    res_hint = func_id if func_id in self.classes else "Any"
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                if enforce_allowlist:
                    suggestion = self._call_allowlist_suggestion(func_id, module_name)
                    if suggestion:
                        alternative = f"use {suggestion}"
                    else:
                        alternative = (
                            "import from an allowlisted module (see docs/spec/"
                            "0015_STDLIB_COMPATIBILITY_MATRIX.md)"
                        )
                    detail = (
                        "Tier 0 only allows direct calls to allowlisted module-level"
                        " functions; rebinding/monkey-patching is not observed"
                    )
                    if suggestion:
                        detail = f"{detail}. warning: allowlisted path is {suggestion}"
                    if self.fallback_policy == "bridge":
                        self.compat.bridge_unavailable(
                            node,
                            f"call to non-allowlisted function '{func_id}'",
                            impact="high",
                            alternative=alternative,
                            detail=detail,
                        )
                        callee = self.visit(node.func)
                        if callee is None:
                            raise NotImplementedError("Unsupported call target")
                        res = MoltValue(self.next_var(), type_hint="Any")
                        if needs_bind:
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                        else:
                            args = self._emit_call_args(node.args)
                            self.emit(
                                MoltOp(
                                    kind="CALL_FUNC",
                                    args=[callee] + args,
                                    result=res,
                                )
                            )
                        return res
                    raise self.compat.unsupported(
                        node,
                        f"call to non-allowlisted function '{func_id}'",
                        impact="high",
                        alternative=alternative,
                        detail=detail,
                    )

        if isinstance(node.func, ast.Name):
            func_id = node.func.id
            imported_from = self.imported_names.get(func_id)
            target_info = self.locals.get(func_id) or self.globals.get(func_id)
            is_local = func_id in self.locals or func_id in self.boxed_locals
            if self.is_async() and func_id in self.async_locals:
                loaded = self._load_local_value(func_id)
                if loaded is not None:
                    target_info = loaded
                is_local = True
            if is_local:
                imported_from = None
            if imported_from:
                normalized = self._normalize_allowlist_module(imported_from)
                allowlist_key = normalized or imported_from
                if func_id == "field" and allowlist_key == "dataclasses":
                    return self._emit_dataclasses_field_call(allowlist_key, node)
            if (
                target_info is None
                and self.current_func_name != "molt_main"
                and self.module_declared_funcs.get(func_id) == "sync"
            ):
                func_symbol = self._function_symbol_for_reference(func_id)
                target_info = MoltValue(func_id, type_hint=f"Func:{func_symbol}")
            if (
                func_id == "sleep"
                and target_info is not None
                and target_info.type_hint == "Func:asyncio__sleep"
            ):
                return self._emit_asyncio_sleep(node.args, node.keywords)
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
                if node.keywords:
                    self._bridge_fallback(
                        node,
                        f"{func_id} with keywords",
                        impact="medium",
                        alternative=f"{func_id} with positional arguments only",
                        detail="keywords are not supported for exception constructors",
                    )
                    return None
                args: list[MoltValue] = []
                for arg in node.args:
                    arg_val = self.visit(arg)
                    if arg_val is None:
                        self._bridge_fallback(
                            node,
                            f"{func_id} with unsupported arg expression",
                            impact="medium",
                            alternative=f"{func_id} with simple literals",
                            detail="argument expression could not be lowered",
                        )
                        return None
                    args.append(arg_val)
                return self._emit_exception_new_from_args(func_id, args)
            if func_id == "globals":
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"globals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_globals_dict()
            if func_id == "locals":
                if node.args or node.keywords:
                    count = len(node.args) + len(node.keywords)
                    msg = f"locals() takes no arguments ({count} given)"
                    return self._emit_type_error_value(msg, "dict")
                return self._emit_locals_dict()
            if func_id == "vars":
                if node.keywords:
                    return self._emit_type_error_value(
                        "vars() takes no keyword arguments", "dict"
                    )
                if len(node.args) > 1:
                    msg = f"vars() takes at most 1 argument ({len(node.args)} given)"
                    return self._emit_type_error_value(msg, "dict")
                if not node.args:
                    return self._emit_locals_dict()
                obj = self.visit(node.args[0])
                if obj is None:
                    raise NotImplementedError("vars expects a simple object")
                callee = self._emit_builtin_function("vars")
                res = MoltValue(self.next_var(), type_hint="dict")
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee, obj], result=res))
                return res
            if func_id == "dir":
                if node.keywords:
                    return self._emit_type_error_value(
                        "dir() takes no keyword arguments", "list"
                    )
                if len(node.args) > 1:
                    msg = f"dir() takes at most 1 argument ({len(node.args)} given)"
                    return self._emit_type_error_value(msg, "list")
                if not node.args:
                    locals_dict = self._emit_locals_dict()
                    keys = MoltValue(self.next_var(), type_hint="dict_keys")
                    self.emit(MoltOp(kind="DICT_KEYS", args=[locals_dict], result=keys))
                    callee = self._emit_builtin_function("sorted")
                    key_none = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_none))
                    reverse_false = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=reverse_false)
                    )
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[callee, keys, key_none, reverse_false],
                            result=res,
                        )
                    )
                    return res
                obj = self.visit(node.args[0])
                if obj is None:
                    raise NotImplementedError("dir expects a simple object")
                callee = self._emit_builtin_function("dir")
                res = MoltValue(self.next_var(), type_hint="list")
                self.emit(MoltOp(kind="CALL_FUNC", args=[callee, obj], result=res))
                return res
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
                                return res
                            else:
                                obj_name = None
                                assume_exact = False
                                if isinstance(node.args[0], ast.Name):
                                    obj_name = node.args[0].id
                                    assume_exact = (
                                        self.exact_locals.get(obj_name) == obj.type_hint
                                    )
                                return self._emit_guarded_getattr(
                                    obj,
                                    name_lit,
                                    obj.type_hint,
                                    assume_exact=assume_exact,
                                    obj_name=obj_name,
                                )
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
                attr_name = None
                if isinstance(node.args[1], ast.Constant) and isinstance(
                    node.args[1].value, str
                ):
                    attr_name = node.args[1].value
                if attr_name:
                    obj_name = None
                    exact_class = None
                    if isinstance(node.args[0], ast.Name):
                        obj_name = node.args[0].id
                        exact_class = self.exact_locals.get(obj_name)
                    if exact_class is not None:
                        self._record_instance_attr_mutation(exact_class, attr_name)
                    elif obj.type_hint in self.classes:
                        self._record_instance_attr_mutation(obj.type_hint, attr_name)
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
                                    assume_exact = (
                                        exact_class is not None
                                        and exact_class == obj.type_hint
                                    )
                                    self._emit_guarded_setattr(
                                        obj,
                                        attr_name,
                                        val,
                                        obj.type_hint,
                                        obj_name=obj_name,
                                        assume_exact=assume_exact,
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
                    attr_name = node.args[1].value
                    exact_class = None
                    if isinstance(node.args[0], ast.Name):
                        exact_class = self.exact_locals.get(node.args[0].id)
                    if exact_class is not None:
                        self._record_instance_attr_mutation(exact_class, attr_name)
                    elif obj.type_hint in self.classes:
                        self._record_instance_attr_mutation(obj.type_hint, attr_name)
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
                return self._emit_open_call(node)
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
                needs_bind = self._call_needs_bind(node)
                if needs_bind:
                    callargs, saw_name_error = self._emit_print_call_args_builder(node)
                    if saw_name_error:
                        return None
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                if len(node.args) == 0:
                    self.emit(
                        MoltOp(kind="PRINT_NEWLINE", args=[], result=MoltValue("none"))
                    )
                    return None
                args: list[MoltValue] = []
                saw_name_error = False
                for expr in node.args:
                    arg = self.visit(expr)
                    if arg is None:
                        if isinstance(expr, ast.Name):
                            exc_val = self._emit_exception_new(
                                "NameError", f"name '{expr.id}' is not defined"
                            )
                            self.emit(
                                MoltOp(
                                    kind="RAISE",
                                    args=[exc_val],
                                    result=MoltValue("none"),
                                )
                            )
                            saw_name_error = True
                            arg = MoltValue(self.next_var(), type_hint="None")
                            self.emit(MoltOp(kind="CONST_NONE", args=[], result=arg))
                        else:
                            raise NotImplementedError("Unsupported call argument")
                    args.append(arg)
                if saw_name_error:
                    return None
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
            elif func_id == "molt_cancel_token_new":
                if node.keywords or len(node.args) > 1:
                    raise NotImplementedError(
                        "molt_cancel_token_new expects 0 or 1 argument"
                    )
                if node.args:
                    parent = self.visit(node.args[0])
                    if parent is None:
                        raise NotImplementedError(
                            "Unsupported parent in molt_cancel_token_new"
                        )
                else:
                    parent = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=parent))
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CANCEL_TOKEN_NEW", args=[parent], result=res))
                return res
            elif func_id == "molt_cancel_token_clone":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_clone expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_CLONE", args=[token], result=res))
                return res
            elif func_id == "molt_cancel_token_drop":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_drop expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_DROP", args=[token], result=res))
                return res
            elif func_id == "molt_cancel_token_cancel":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_cancel expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_TOKEN_CANCEL", args=[token], result=res))
                return res
            elif func_id == "molt_future_cancel":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_future_cancel expects 1 argument")
                future = self.visit(node.args[0])
                if future is None:
                    raise NotImplementedError("Unsupported future")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FUTURE_CANCEL", args=[future], result=res))
                return res
            elif func_id == "molt_future_cancel_msg":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_future_cancel_msg expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                msg = self.visit(node.args[1])
                if future is None or msg is None:
                    raise NotImplementedError("Unsupported future cancel message")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="FUTURE_CANCEL_MSG", args=[future, msg], result=res)
                )
                return res
            elif func_id == "molt_future_cancel_clear":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_future_cancel_clear expects 1 argument"
                    )
                future = self.visit(node.args[0])
                if future is None:
                    raise NotImplementedError("Unsupported future cancel clear")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="FUTURE_CANCEL_CLEAR", args=[future], result=res))
                return res
            elif func_id == "molt_promise_new":
                if node.keywords or node.args:
                    raise NotImplementedError("molt_promise_new expects no arguments")
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(MoltOp(kind="PROMISE_NEW", args=[], result=res))
                return res
            elif func_id == "molt_promise_set_result":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_promise_set_result expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                result = self.visit(node.args[1])
                if future is None or result is None:
                    raise NotImplementedError("Unsupported promise set result")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="PROMISE_SET_RESULT", args=[future, result], result=res)
                )
                return res
            elif func_id == "molt_promise_set_exception":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_promise_set_exception expects 2 arguments"
                    )
                future = self.visit(node.args[0])
                exc = self.visit(node.args[1])
                if future is None or exc is None:
                    raise NotImplementedError("Unsupported promise set exception")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(kind="PROMISE_SET_EXCEPTION", args=[future, exc], result=res)
                )
                return res
            elif func_id == "molt_task_register_token_owned":
                if node.keywords or len(node.args) != 2:
                    raise NotImplementedError(
                        "molt_task_register_token_owned expects 2 arguments"
                    )
                task = self.visit(node.args[0])
                token = self.visit(node.args[1])
                if task is None or token is None:
                    raise NotImplementedError("Unsupported task token registration")
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="TASK_REGISTER_TOKEN_OWNED",
                        args=[task, token],
                        result=res,
                    )
                )
                return res
            elif func_id == "molt_cancel_token_is_cancelled":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_is_cancelled expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="CANCEL_TOKEN_IS_CANCELLED", args=[token], result=res)
                )
                return res
            elif func_id == "molt_cancel_token_set_current":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError(
                        "molt_cancel_token_set_current expects 1 argument"
                    )
                token = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(kind="CANCEL_TOKEN_SET_CURRENT", args=[token], result=res)
                )
                return res
            elif func_id == "molt_cancel_token_get_current":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_cancel_token_get_current expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CANCEL_TOKEN_GET_CURRENT", args=[], result=res))
                return res
            elif func_id == "molt_cancelled":
                if node.keywords or node.args:
                    raise NotImplementedError("molt_cancelled expects no arguments")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CANCELLED", args=[], result=res))
                return res
            elif func_id == "molt_cancel_current":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_cancel_current expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CANCEL_CURRENT", args=[], result=res))
                return res
            elif func_id == "molt_block_on":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_block_on expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="ASYNC_BLOCK_ON", args=[arg], result=res))
                return res
            elif func_id == "molt_asyncgen_shutdown":
                if node.keywords or node.args:
                    raise NotImplementedError(
                        "molt_asyncgen_shutdown expects no arguments"
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="ASYNCGEN_SHUTDOWN", args=[], result=res))
                return res
            elif func_id == "molt_async_sleep":
                if node.keywords or len(node.args) > 2:
                    raise NotImplementedError("molt_async_sleep expects 0-2 arguments")
                args = []
                if node.args:
                    delay_val = self.visit(node.args[0])
                    if delay_val is None:
                        raise NotImplementedError(
                            "Unsupported delay in molt_async_sleep"
                        )
                    args.append(delay_val)
                if len(node.args) == 2:
                    result_val = self.visit(node.args[1])
                    if result_val is None:
                        raise NotImplementedError(
                            "Unsupported result in molt_async_sleep"
                        )
                    args.append(result_val)
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="CALL_ASYNC", args=["molt_async_sleep", *args], result=res
                    )
                )
                return res
            elif func_id == "molt_thread_submit":
                if node.keywords or len(node.args) != 3:
                    raise NotImplementedError("molt_thread_submit expects 3 arguments")
                callable_val = self.visit(node.args[0])
                args_val = self.visit(node.args[1])
                kwargs_val = self.visit(node.args[2])
                if callable_val is None or args_val is None or kwargs_val is None:
                    raise NotImplementedError("Unsupported thread submit arguments")
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(
                    MoltOp(
                        kind="THREAD_SUBMIT",
                        args=[callable_val, args_val, kwargs_val],
                        result=res,
                    )
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
            elif func_id == "molt_chan_drop":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("molt_chan_drop expects 1 argument")
                chan = self.visit(node.args[0])
                if chan is None:
                    raise NotImplementedError("Unsupported channel handle")
                self.emit(
                    MoltOp(kind="CHAN_DROP", args=[chan], result=MoltValue("none"))
                )
                return None
            class_id = None
            if func_id in self.classes:
                class_id = func_id
            if class_id is not None:
                class_info = self.classes[class_id]
                if imported_from:
                    class_ref = self._emit_module_attr_get_on(imported_from, class_id)
                else:
                    local_class = self._load_local_value(class_id)
                    if local_class is not None:
                        class_ref = local_class
                    else:
                        class_ref = self._emit_module_attr_get(class_id)
                if self._class_is_exception_subclass(class_id, class_info):
                    new_method = class_info.get("methods", {}).get("__new__")
                    if new_method is None:
                        for base_name in class_info.get("mro", [])[1:]:
                            base_info = self.classes.get(base_name)
                            if base_info and base_info.get("methods", {}).get(
                                "__new__"
                            ):
                                new_method = base_info["methods"]["__new__"]
                                break
                    if needs_bind or new_method is not None:
                        callargs = self._emit_call_args_builder(node)
                        res = MoltValue(self.next_var(), type_hint="exception")
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[class_ref, callargs],
                                result=res,
                            )
                        )
                        return res
                    args = self._emit_call_args(node.args)
                    res = self._emit_exception_new_from_class(class_ref, args)
                    init_method = class_info.get("methods", {}).get("__init__")
                    if init_method is None:
                        for base_name in class_info.get("mro", [])[1:]:
                            base_info = self.classes.get(base_name)
                            if base_info and base_info.get("methods", {}).get(
                                "__init__"
                            ):
                                init_method = base_info["methods"]["__init__"]
                                break
                    if init_method is not None:
                        init_func = init_method["func"]
                        target_name = init_func.type_hint.split(":", 1)[1]
                        init_args = [res] + args
                        func_obj = None
                        param_count = init_method.get("param_count")
                        defaults = init_method.get("defaults", [])
                        if param_count is not None:
                            missing = param_count - len(init_args)
                            if missing > 0 and any(
                                not spec.get("const", False)
                                for spec in defaults[-missing:]
                            ):
                                func_obj = self._emit_class_method_func(
                                    class_ref, "__init__"
                                )
                        init_args = self._apply_default_specs(
                            param_count,
                            defaults,
                            init_args,
                            node,
                            call_name=f"{class_id}.__init__",
                            func_obj=func_obj,
                        )
                        init_res = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="CALL",
                                args=[target_name] + init_args,
                                result=init_res,
                            )
                        )
                    return res
                if class_info.get("dataclass"):
                    if any(kw.arg is None for kw in node.keywords):
                        raise NotImplementedError(
                            "Dataclass **kwargs are not supported"
                        )
                    if any(isinstance(arg, ast.Starred) for arg in node.args):
                        raise NotImplementedError("Dataclass *args are not supported")
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
                    self.emit(
                        MoltOp(
                            kind="DATACLASS_SET_CLASS",
                            args=[res, class_ref],
                            result=MoltValue("none"),
                        )
                    )
                    return res
                # TODO(semantics, owner:frontend, milestone:TC2, priority:P1, status:partial): honor __new__ overrides for non-exception classes.
                res = MoltValue(self.next_var(), type_hint=class_id)
                alloc_kind = "ALLOC_CLASS"
                if not class_info.get("dynamic"):
                    alloc_kind = (
                        "ALLOC_CLASS_STATIC"
                        if class_info.get("static")
                        else "ALLOC_CLASS_TRUSTED"
                    )
                self.emit(
                    MoltOp(kind=alloc_kind, args=[class_ref, class_id], result=res)
                )
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
                    use_init = False
                    if not class_info.get("dynamic"):
                        if default_expr is None:
                            use_init = True
                        elif isinstance(default_expr, ast.Constant):
                            const_val = default_expr.value
                            if const_val is None or isinstance(
                                const_val, (int, float, bool)
                            ):
                                use_init = True
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
                    elif use_init:
                        self._emit_guarded_setattr(
                            res, name, val, class_id, use_init=True, assume_exact=True
                        )
                    else:
                        self._emit_guarded_setattr(
                            res, name, val, class_id, assume_exact=True
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
                    if needs_bind:
                        if self.current_func_name != "molt_main":
                            init_func = self._emit_class_method_func(
                                class_ref, "__init__"
                            )
                        bound_init = MoltValue(self.next_var(), type_hint="method")
                        self.emit(
                            MoltOp(
                                kind="BOUND_METHOD_NEW",
                                args=[init_func, res],
                                result=bound_init,
                            )
                        )
                        callargs = self._emit_call_args_builder(node)
                        init_res = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[bound_init, callargs],
                                result=init_res,
                            )
                        )
                        self.emit(
                            MoltOp(
                                kind="OBJECT_SET_CLASS",
                                args=[res, class_ref],
                                result=MoltValue("none"),
                            )
                        )
                        return res
                    target_name = init_func.type_hint.split(":", 1)[1]
                    args = [res] + self._emit_call_args(node.args)
                    func_obj = None
                    param_count = init_method.get("param_count")
                    defaults = init_method.get("defaults", [])
                    if param_count is not None:
                        missing = param_count - len(args)
                        if missing > 0 and any(
                            not spec.get("const", False) for spec in defaults[-missing:]
                        ):
                            func_obj = self._emit_class_method_func(
                                class_ref, "__init__"
                            )
                    args = self._apply_default_specs(
                        param_count,
                        defaults,
                        args,
                        node,
                        call_name=f"{class_id}.__init__",
                        func_obj=func_obj,
                    )
                    init_res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=init_res)
                    )
                    self.emit(
                        MoltOp(
                            kind="OBJECT_SET_CLASS",
                            args=[res, class_ref],
                            result=MoltValue("none"),
                        )
                    )
                    return res
                self.emit(
                    MoltOp(
                        kind="OBJECT_SET_CLASS",
                        args=[res, class_ref],
                        result=MoltValue("none"),
                    )
                )
                callargs = self._emit_call_args_builder(node)
                init_name = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=["__init__"], result=init_name))
                init_obj = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="GETATTR_NAME",
                        args=[class_ref, init_name],
                        result=init_obj,
                    )
                )
                bound_init = MoltValue(self.next_var(), type_hint="method")
                self.emit(
                    MoltOp(
                        kind="BOUND_METHOD_NEW",
                        args=[init_obj, res],
                        result=bound_init,
                    )
                )
                init_res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_BIND", args=[bound_init, callargs], result=init_res
                    )
                )
                return res

            if target_info and str(target_info.type_hint).startswith(
                ("AsyncFunc:", "AsyncClosureFunc:")
            ):
                target_value = target_info
                if needs_bind:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                parts = target_info.type_hint.split(":")
                func_kind = parts[0]
                poll_func = parts[1]
                closure_size = int(parts[2])
                func_symbol = (
                    poll_func[: -len("_poll")]
                    if poll_func.endswith("_poll")
                    else poll_func
                )
                args, _ = self._emit_direct_call_args_for_symbol(func_symbol, node)
                res = MoltValue(self.next_var(), type_hint="Future")
                if func_kind == "AsyncClosureFunc":
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[target_value] + args, result=res)
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="ALLOC_TASK",
                            args=[poll_func, closure_size] + args,
                            result=res,
                            metadata={"task_kind": "future"},
                        )
                    )
                return res
            if target_info and str(target_info.type_hint).startswith(
                ("AsyncGenFunc:", "AsyncGenClosureFunc:")
            ):
                target_value = target_info
                if needs_bind:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="async_generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                parts = target_info.type_hint.split(":")
                func_kind = parts[0]
                poll_func = parts[1]
                closure_size = int(parts[2])
                func_symbol = (
                    poll_func[: -len("_poll")]
                    if poll_func.endswith("_poll")
                    else poll_func
                )
                args, _ = self._emit_direct_call_args_for_symbol(func_symbol, node)
                res = MoltValue(self.next_var(), type_hint="async_generator")
                if func_kind == "AsyncGenClosureFunc":
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[target_value] + args, result=res)
                    )
                else:
                    gen_val = MoltValue(self.next_var(), type_hint="generator")
                    self.emit(
                        MoltOp(
                            kind="ALLOC_TASK",
                            args=[poll_func, closure_size] + args,
                            result=gen_val,
                            metadata={"task_kind": "generator"},
                        )
                    )
                    self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
                return res
            if target_info and str(target_info.type_hint).startswith(
                ("GenFunc:", "GenClosureFunc:")
            ):
                target_value = target_info
                if needs_bind:
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint="generator")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_value, callargs],
                            result=res,
                        )
                    )
                    return res
                parts = target_info.type_hint.split(":")
                func_kind = parts[0]
                poll_func = parts[1]
                closure_size = int(parts[2])
                func_symbol = (
                    poll_func[: -len("_poll")]
                    if poll_func.endswith("_poll")
                    else poll_func
                )
                args, _ = self._emit_direct_call_args_for_symbol(func_symbol, node)
                if func_kind == "GenClosureFunc":
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        target_value = self._emit_module_attr_get(func_id)
                    closure_val = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(
                            kind="FUNCTION_CLOSURE_BITS",
                            args=[target_value],
                            result=closure_val,
                        )
                    )
                    args = [closure_val] + args
                res = MoltValue(self.next_var(), type_hint="generator")
                self.emit(
                    MoltOp(
                        kind="ALLOC_TASK",
                        args=[poll_func, closure_size] + args,
                        result=res,
                        metadata={"task_kind": "generator"},
                    )
                )
                return res

            if target_info and str(target_info.type_hint).startswith("BoundMethod:"):
                res_hint = "Any"
                class_name = "Unknown"
                method_name = "method"
                method_info = None
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
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    res = MoltValue(self.next_var(), type_hint=res_hint)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[target_info, callargs],
                            result=res,
                        )
                    )
                    return res
                args = self._emit_call_args(node.args)
                if method_info:
                    func_obj = None
                    param_count = method_info.get("param_count")
                    defaults = method_info.get("defaults", [])
                    if param_count is not None:
                        missing = param_count - (len(args) + 1)
                        if missing > 0 and any(
                            not spec.get("const", False) for spec in defaults[-missing:]
                        ):
                            func_obj = self._emit_bound_method_func(target_info)
                    args = self._apply_default_specs(
                        param_count,
                        defaults,
                        args,
                        node,
                        call_name=f"{class_name}.{method_name}",
                        func_obj=func_obj,
                        implicit_self=True,
                    )
                res = MoltValue(self.next_var(), type_hint=res_hint)
                self.emit(
                    MoltOp(kind="CALL_METHOD", args=[target_info] + args, result=res)
                )
                return res

            if target_info and str(target_info.type_hint).startswith("Func:"):
                target_name = target_info.type_hint.split(":")[1]
                direct_ok = target_name in self.func_default_specs
                if not direct_ok:
                    func_name = self.func_symbol_names.get(target_name)
                    if func_name and self._lookup_func_defaults(None, func_name):
                        direct_ok = True
                if needs_bind or not direct_ok:
                    callargs = self._emit_call_args_builder(node)
                    callee = target_info
                    if (
                        self.current_func_name != "molt_main"
                        and func_id not in self.locals
                        and func_id not in self.async_locals
                    ):
                        callee = self._emit_module_attr_get(func_id)
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res
                args, func_obj = self._emit_direct_call_args_for_symbol(
                    target_name, node
                )
                res = MoltValue(self.next_var(), type_hint="int")
                if self.is_async() or (
                    isinstance(node.func, ast.Name)
                    and node.func.id in self.stable_module_funcs
                ):
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                else:
                    callee = func_obj or self.visit(node.func)
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

            if target_info is not None and func_id in self.locals:
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                if needs_bind:
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                else:
                    if isinstance(
                        callee.type_hint, str
                    ) and callee.type_hint.startswith("Func:"):
                        func_symbol = callee.type_hint.split(":", 1)[1]
                        if func_symbol not in self.func_default_specs:
                            res = MoltValue(self.next_var(), type_hint="Any")
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        args, func_obj = self._emit_direct_call_args_for_symbol(
                            func_symbol, node, func_obj=callee
                        )
                        res = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(
                                kind="CALL_GUARDED",
                                args=[func_obj or callee] + args,
                                result=res,
                                metadata={"target": func_symbol},
                            )
                        )
                        return res
                    if isinstance(
                        callee.type_hint, str
                    ) and callee.type_hint.startswith("ClosureFunc:"):
                        func_symbol = callee.type_hint.split(":", 1)[1]
                        if func_symbol not in self.func_default_specs:
                            res = MoltValue(self.next_var(), type_hint="Any")
                            callargs = self._emit_call_args_builder(node)
                            self.emit(
                                MoltOp(
                                    kind="CALL_BIND",
                                    args=[callee, callargs],
                                    result=res,
                                )
                            )
                            return res
                        args, _ = self._emit_direct_call_args_for_symbol(
                            func_symbol, node, func_obj=callee
                        )
                        res = MoltValue(self.next_var(), type_hint="Any")
                        self.emit(
                            MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                        )
                        return res
                    args = self._emit_call_args(node.args)
                    if imported_from:
                        args = self._apply_direct_call_defaults(
                            imported_from, func_id, args, node
                        )
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
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
                if node.keywords:
                    raise NotImplementedError("len does not support keywords")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[arg], result=res))
                return res
            if func_id == "id":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("id expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported id argument")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ID", args=[arg], result=res))
                return res
            if func_id == "bool":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=res))
                    return res
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported bool argument")
                neg = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[arg], result=neg))
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="NOT", args=[neg], result=res))
                return res
            if func_id == "ord":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("ord expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported ord argument")
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="ORD", args=[arg], result=res))
                return res
            if func_id == "chr":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("chr expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported chr argument")
                res = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CHR", args=[arg], result=res))
                return res
            if func_id == "repr":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("repr expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported repr argument")
                return self._emit_repr_from_obj(arg)
            if func_id == "callable":
                if node.keywords or len(node.args) != 1:
                    raise NotImplementedError("callable expects 1 argument")
                arg = self.visit(node.args[0])
                if arg is None:
                    raise NotImplementedError("Unsupported callable argument")
                res = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="IS_CALLABLE", args=[arg], result=res))
                return res
            if func_id == "str":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
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
                if node.keywords or len(node.args) not in (1, 2, 3):
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                range_args = self._parse_range_call(node)
                if range_args is None:
                    # TODO(type-coverage, owner:frontend, milestone:TC2, priority:P2, status:partial): accept range arguments that currently fail lowering (e.g., oversized ints).
                    raise NotImplementedError("Unsupported range invocation")
                start, stop, step = range_args
                res = MoltValue(self.next_var(), type_hint="range")
                self.emit(
                    MoltOp(kind="RANGE_NEW", args=[start, stop, step], result=res)
                )
                return res
            if func_id == "enumerate":
                if len(node.args) > 2:
                    raise NotImplementedError("enumerate expects 1 or 2 arguments")
                if node.keywords:
                    for keyword in node.keywords:
                        if keyword.arg is None:
                            raise NotImplementedError(
                                "enumerate does not support **kwargs"
                            )
                        if keyword.arg != "start":
                            raise NotImplementedError(
                                f"enumerate got unexpected keyword {keyword.arg}"
                            )
                iterable = self.visit(node.args[0]) if node.args else None
                if iterable is None:
                    raise NotImplementedError("Unsupported enumerate iterable")
                start_val = None
                has_start = False
                if len(node.args) == 2:
                    start_val = self.visit(node.args[1])
                    has_start = True
                for keyword in node.keywords:
                    if keyword.arg == "start":
                        if has_start:
                            raise NotImplementedError(
                                "enumerate got multiple values for start"
                            )
                        start_val = self.visit(keyword.value)
                        has_start = True
                if start_val is None:
                    start_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
                has_start_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(
                    MoltOp(kind="CONST_BOOL", args=[has_start], result=has_start_val)
                )
                res = MoltValue(self.next_var(), type_hint="iter")
                self.emit(
                    MoltOp(
                        kind="ENUMERATE",
                        args=[iterable, start_val, has_start_val],
                        result=res,
                    )
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
                if node.keywords or len(node.args) not in (1, 2):
                    raise NotImplementedError(
                        "anext expects 1 or 2 positional arguments"
                    )
                iter_obj = self.visit(node.args[0])
                if iter_obj is None:
                    raise NotImplementedError("Unsupported iterator in anext()")
                if len(node.args) == 2:
                    default_val = self.visit(node.args[1])
                    if default_val is None:
                        raise NotImplementedError("Unsupported default in anext()")
                    placeholder = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=placeholder))
                    res = MoltValue(self.next_var(), type_hint="Future")
                    self.emit(
                        MoltOp(
                            kind="CALL_ASYNC",
                            args=[
                                "molt_anext_default_poll",
                                iter_obj,
                                default_val,
                                placeholder,
                            ],
                            result=res,
                        )
                    )
                    return res
                res = MoltValue(self.next_var(), type_hint="Future")
                self.emit(MoltOp(kind="ANEXT", args=[iter_obj], result=res))
                return res
            if func_id == "next":
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("next expects 1 or 2 arguments")
                iter_obj = self.visit(node.args[0])
                if iter_obj is None:
                    raise NotImplementedError("Unsupported iterator in next()")
                pair = self._emit_iter_next_checked(iter_obj)
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
            if func_id == "sum":
                if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                    kw.arg is None for kw in node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if needs_bind:
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND", args=[callee, callargs], result=res
                            )
                        )
                    else:
                        args = self._emit_call_args(node.args)
                        self.emit(
                            MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                        )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        "sum expected at least 1 argument, got 0"
                    )
                if len(node.args) > 2:
                    return self._emit_type_error_value(
                        f"sum expected at most 2 arguments, got {len(node.args)}"
                    )
                start_expr = None
                has_start = False
                if len(node.args) == 2:
                    start_expr = node.args[1]
                    has_start = True
                for keyword in node.keywords:
                    if keyword.arg != "start":
                        msg = (
                            f"sum() got an unexpected keyword argument '{keyword.arg}'"
                        )
                        return self._emit_type_error_value(msg)
                    if has_start:
                        return self._emit_type_error_value(
                            "sum() got multiple values for argument 'start'"
                        )
                    start_expr = keyword.value
                    has_start = True
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported sum iterable")
                if start_expr is None:
                    start_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=start_val))
                else:
                    start_val = self.visit(start_expr)
                    if start_val is None:
                        raise NotImplementedError("Unsupported sum start value")
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC", args=[callee, iterable, start_val], result=res
                    )
                )
                return res
            if func_id == "map":
                if (
                    any(isinstance(arg, ast.Starred) for arg in node.args)
                    or node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                if len(node.args) < 2:
                    return self._emit_type_error_value(
                        "map() must have at least two arguments"
                    )
                func_val = self.visit(node.args[0])
                if func_val is None:
                    raise NotImplementedError("Unsupported map function")
                iter_vals: list[MoltValue] = []
                for expr in node.args[1:]:
                    iter_val = self.visit(expr)
                    if iter_val is None:
                        raise NotImplementedError("Unsupported map iterable")
                    iter_vals.append(iter_val)
                iter_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=iter_vals, result=iter_tuple))
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee, func_val, iter_tuple],
                        result=res,
                    )
                )
                return res
            if func_id == "zip":
                if (
                    any(isinstance(arg, ast.Starred) for arg in node.args)
                    or node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                    return res
                iter_vals: list[MoltValue] = []
                for expr in node.args:
                    iter_val = self.visit(expr)
                    if iter_val is None:
                        raise NotImplementedError("Unsupported zip iterable")
                    iter_vals.append(iter_val)
                iter_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=iter_vals, result=iter_tuple))
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="CALL_FUNC", args=[callee, iter_tuple], result=res)
                )
                return res
            if func_id in {"min", "max"}:
                if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                    kw.arg is None for kw in node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if needs_bind:
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND", args=[callee, callargs], result=res
                            )
                        )
                    else:
                        args = self._emit_call_args(node.args)
                        self.emit(
                            MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                        )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        f"{func_id} expected at least 1 argument, got 0"
                    )
                key_expr = None
                default_expr = None
                for keyword in node.keywords:
                    if keyword.arg not in {"key", "default"}:
                        msg = (
                            f"{func_id}() got an unexpected keyword argument "
                            f"'{keyword.arg}'"
                        )
                        return self._emit_type_error_value(msg)
                    if keyword.arg == "key":
                        if key_expr is not None:
                            return self._emit_type_error_value(
                                f"{func_id}() got multiple values for argument 'key'"
                            )
                        key_expr = keyword.value
                    else:
                        if default_expr is not None:
                            return self._emit_type_error_value(
                                f"{func_id}() got multiple values for argument 'default'"
                            )
                        default_expr = keyword.value
                if len(node.args) > 1 and default_expr is not None:
                    msg = (
                        f"Cannot specify a default for {func_id}() with "
                        "multiple positional arguments"
                    )
                    return self._emit_type_error_value(msg)
                arg_vals: list[MoltValue] = []
                for expr in node.args:
                    arg_val = self.visit(expr)
                    if arg_val is None:
                        raise NotImplementedError(
                            f"Unsupported {func_id} positional argument"
                        )
                    arg_vals.append(arg_val)
                args_tuple = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(MoltOp(kind="TUPLE_NEW", args=arg_vals, result=args_tuple))
                if key_expr is None:
                    key_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_val))
                else:
                    key_val = self.visit(key_expr)
                    if key_val is None:
                        raise NotImplementedError(f"Unsupported {func_id} key")
                if default_expr is None:
                    default_val = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(MoltOp(kind="MISSING", args=[], result=default_val))
                else:
                    default_val = self.visit(default_expr)
                    if default_val is None:
                        raise NotImplementedError(f"Unsupported {func_id} default")
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee, args_tuple, key_val, default_val],
                        result=res,
                    )
                )
                return res
            if func_id == "sorted":
                if any(isinstance(arg, ast.Starred) for arg in node.args) or any(
                    kw.arg is None for kw in node.keywords
                ):
                    callee = self._emit_builtin_function(func_id)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    if needs_bind:
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND", args=[callee, callargs], result=res
                            )
                        )
                    else:
                        args = self._emit_call_args(node.args)
                        self.emit(
                            MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                        )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        "sorted expected 1 argument, got 0"
                    )
                if len(node.args) > 1:
                    return self._emit_type_error_value(
                        f"sorted expected 1 argument, got {len(node.args)}"
                    )
                key_expr = None
                reverse_expr = None
                for keyword in node.keywords:
                    if keyword.arg not in {"key", "reverse"}:
                        msg = (
                            "sorted() got an unexpected keyword argument "
                            f"'{keyword.arg}'"
                        )
                        return self._emit_type_error_value(msg)
                    if keyword.arg == "key":
                        if key_expr is not None:
                            return self._emit_type_error_value(
                                "sorted() got multiple values for argument 'key'"
                            )
                        key_expr = keyword.value
                    else:
                        if reverse_expr is not None:
                            return self._emit_type_error_value(
                                "sorted() got multiple values for argument 'reverse'"
                            )
                        reverse_expr = keyword.value
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported sorted iterable")
                if key_expr is None:
                    key_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=key_val))
                else:
                    key_val = self.visit(key_expr)
                    if key_val is None:
                        raise NotImplementedError("Unsupported sorted key")
                if reverse_expr is None:
                    reverse_val = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=reverse_val)
                    )
                else:
                    reverse_val = self.visit(reverse_expr)
                    if reverse_val is None:
                        raise NotImplementedError("Unsupported sorted reverse")
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC",
                        args=[callee, iterable, key_val, reverse_val],
                        result=res,
                    )
                )
                return res
            if func_id == "iter":
                if node.keywords:
                    return self._emit_type_error_value(
                        "iter() takes no keyword arguments", "iter"
                    )
                if len(node.args) == 1:
                    iterable = self.visit(node.args[0])
                    if iterable is None:
                        raise NotImplementedError("Unsupported iterable in iter()")
                    return self._emit_iter_new(iterable)
                if len(node.args) == 2:
                    callable_val = self.visit(node.args[0])
                    sentinel_val = self.visit(node.args[1])
                    if callable_val is None or sentinel_val is None:
                        raise NotImplementedError("Unsupported iter() arguments")
                    callee = MoltValue(self.next_var(), type_hint="function")
                    self.emit(
                        MoltOp(
                            kind="BUILTIN_FUNC",
                            args=["molt_iter_sentinel", 2],
                            result=callee,
                        )
                    )
                    self._emit_function_metadata(
                        callee,
                        name="iter",
                        qualname="iter",
                        posonly_params=["callable", "sentinel"],
                        pos_or_kw_params=[],
                        kwonly_params=[],
                        vararg=None,
                        varkw=None,
                        default_exprs=[],
                        kw_default_exprs=[],
                        docstring=None,
                        module_override="builtins",
                    )
                    res = MoltValue(self.next_var(), type_hint="iter")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[callee, callable_val, sentinel_val],
                            result=res,
                        )
                    )
                    return res
                if not node.args:
                    return self._emit_type_error_value(
                        "iter expected 1 argument, got 0", "iter"
                    )
                msg = f"iter expected at most 2 arguments, got {len(node.args)}"
                return self._emit_type_error_value(msg, "iter")
            if func_id == "list":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="list")
                    self.emit(MoltOp(kind="LIST_NEW", args=[], result=res))
                    return res
                range_args = self._parse_range_call(node.args[0])
                if range_args is not None:
                    start, stop, step = range_args
                    return self._emit_range_list(start, stop, step)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported list input")
                return self._emit_list_from_iter(iterable)
            if func_id == "tuple":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
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
            if func_id == "dict":
                if len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                res = MoltValue(self.next_var(), type_hint="dict")
                if not node.args:
                    self.emit(MoltOp(kind="DICT_NEW", args=[], result=res))
                else:
                    iterable = self.visit(node.args[0])
                    if iterable is None:
                        raise NotImplementedError("Unsupported dict input")
                    self.emit(MoltOp(kind="DICT_FROM_OBJ", args=[iterable], result=res))
                for kw in node.keywords:
                    if kw.arg is None:
                        mapping = self.visit(kw.value)
                        if mapping is None:
                            raise NotImplementedError("Unsupported dict ** input")
                        self.emit(
                            MoltOp(
                                kind="DICT_UPDATE_KWSTAR",
                                args=[res, mapping],
                                result=MoltValue("none"),
                            )
                        )
                    else:
                        key = MoltValue(self.next_var(), type_hint="str")
                        self.emit(MoltOp(kind="CONST_STR", args=[kw.arg], result=key))
                        val = self.visit(kw.value)
                        if val is None:
                            raise NotImplementedError("Unsupported dict kw value")
                        self.emit(
                            MoltOp(
                                kind="STORE_INDEX",
                                args=[res, key, val],
                                result=MoltValue("none"),
                            )
                        )
                return res
            if func_id == "float":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="float")
                    self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=res))
                    return res
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported float input")
                res = MoltValue(self.next_var(), type_hint="float")
                self.emit(MoltOp(kind="FLOAT_FROM_OBJ", args=[value], result=res))
                return res
            if func_id == "int":
                if node.keywords:
                    # TODO(type-coverage, owner:frontend, milestone:TC2, priority:P1, status:partial): support int(x=..., base=...) keyword arguments.
                    raise NotImplementedError("int does not support keywords")
                if len(node.args) > 2:
                    raise NotImplementedError("int expects 0-2 arguments")
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[0], result=res))
                    return res
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported int input")
                if len(node.args) == 2:
                    base = self.visit(node.args[1])
                    if base is None:
                        base = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=base))
                    has_base = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=has_base))
                else:
                    base = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=base))
                    has_base = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=has_base))
                res = MoltValue(self.next_var(), type_hint="int")
                self.emit(
                    MoltOp(
                        kind="INT_FROM_OBJ", args=[value, base, has_base], result=res
                    )
                )
                return res
            if func_id == "pow":
                if node.keywords:
                    raise NotImplementedError("pow does not support keywords")
                if len(node.args) not in (2, 3):
                    raise NotImplementedError("pow expects 2 or 3 arguments")
                base = self.visit(node.args[0])
                exp = self.visit(node.args[1])
                if base is None or exp is None:
                    raise NotImplementedError("Unsupported pow inputs")
                if len(node.args) == 2:
                    res_type = (
                        "float"
                        if "float" in {base.type_hint, exp.type_hint}
                        else "Unknown"
                    )
                    res = MoltValue(self.next_var(), type_hint=res_type)
                    self.emit(MoltOp(kind="POW", args=[base, exp], result=res))
                    return res
                mod = self.visit(node.args[2])
                if mod is None:
                    raise NotImplementedError("Unsupported pow mod input")
                int_like = {"int", "bool"}
                res_type = (
                    "int"
                    if {
                        base.type_hint,
                        exp.type_hint,
                        mod.type_hint,
                    }.issubset(int_like)
                    else "Unknown"
                )
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(MoltOp(kind="POW_MOD", args=[base, exp, mod], result=res))
                return res
            if func_id == "round":
                if node.keywords:
                    raise NotImplementedError("round does not support keywords")
                if len(node.args) not in (1, 2):
                    raise NotImplementedError("round expects 1 or 2 arguments")
                value = self.visit(node.args[0])
                if value is None:
                    raise NotImplementedError("Unsupported round input")
                if len(node.args) == 2:
                    ndigits = self.visit(node.args[1])
                    if ndigits is None:
                        ndigits = MoltValue(self.next_var(), type_hint="None")
                        self.emit(MoltOp(kind="CONST_NONE", args=[], result=ndigits))
                    has_ndigits = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[True], result=has_ndigits)
                    )
                    if value.type_hint == "float":
                        res_type = "float"
                    elif value.type_hint in {"int", "bool"}:
                        res_type = "int"
                    else:
                        res_type = "Unknown"
                else:
                    ndigits = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=ndigits))
                    has_ndigits = MoltValue(self.next_var(), type_hint="bool")
                    self.emit(
                        MoltOp(kind="CONST_BOOL", args=[False], result=has_ndigits)
                    )
                    res_type = (
                        "int"
                        if value.type_hint in {"int", "bool", "float"}
                        else "Unknown"
                    )
                res = MoltValue(self.next_var(), type_hint=res_type)
                self.emit(
                    MoltOp(kind="ROUND", args=[value, ndigits, has_ndigits], result=res)
                )
                return res
            if func_id == "set":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="set")
                    self.emit(MoltOp(kind="SET_NEW", args=[], result=res))
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
                    return self._emit_set_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported set input")
                return self._emit_set_from_iter(iterable)
            if func_id == "frozenset":
                if node.keywords or len(node.args) > 1:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                if not node.args:
                    res = MoltValue(self.next_var(), type_hint="frozenset")
                    self.emit(MoltOp(kind="FROZENSET_NEW", args=[], result=res))
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
                    return self._emit_frozenset_from_iter(range_obj)
                iterable = self.visit(node.args[0])
                if iterable is None:
                    raise NotImplementedError("Unsupported frozenset input")
                return self._emit_frozenset_from_iter(iterable)
                return self._emit_tuple_from_iter(iterable)
            if func_id == "bytes":
                if any(kw.arg is None for kw in node.keywords):
                    raise NotImplementedError("bytes does not support **kwargs")
                if len(node.args) > 3:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                source_expr = node.args[0] if node.args else None
                encoding_expr = node.args[1] if len(node.args) > 1 else None
                errors_expr = node.args[2] if len(node.args) > 2 else None
                has_encoding = encoding_expr is not None
                has_errors = errors_expr is not None
                for kw in node.keywords:
                    if kw.arg == "source":
                        if source_expr is not None:
                            return self._emit_type_error_value(
                                "bytes() got multiple values for argument 'source'",
                                "bytes",
                            )
                        source_expr = kw.value
                    elif kw.arg == "encoding":
                        if has_encoding:
                            return self._emit_type_error_value(
                                "bytes() got multiple values for argument 'encoding'",
                                "bytes",
                            )
                        encoding_expr = kw.value
                        has_encoding = True
                    elif kw.arg == "errors":
                        if has_errors:
                            return self._emit_type_error_value(
                                "bytes() got multiple values for argument 'errors'",
                                "bytes",
                            )
                        errors_expr = kw.value
                        has_errors = True
                    else:
                        msg = f"bytes() got an unexpected keyword argument '{kw.arg}'"
                        return self._emit_type_error_value(msg, "bytes")
                if source_expr is None and not has_encoding and not has_errors:
                    res = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=res))
                    return res
                if source_expr is None:
                    source_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=source_val))
                else:
                    source_val = self.visit(source_expr)
                    if source_val is None:
                        raise NotImplementedError("Unsupported bytes input")
                if has_encoding:
                    if encoding_expr is None:
                        raise NotImplementedError("Unsupported bytes encoding")
                    encoding_val = self.visit(encoding_expr)
                    if encoding_val is None:
                        raise NotImplementedError("Unsupported bytes encoding")
                else:
                    encoding_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=encoding_val))
                if has_errors:
                    if errors_expr is None:
                        raise NotImplementedError("Unsupported bytes errors")
                    errors_val = self.visit(errors_expr)
                    if errors_val is None:
                        raise NotImplementedError("Unsupported bytes errors")
                else:
                    errors_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=errors_val))
                res = MoltValue(self.next_var(), type_hint="bytes")
                if has_encoding or has_errors:
                    self.emit(
                        MoltOp(
                            kind="BYTES_FROM_STR",
                            args=[source_val, encoding_val, errors_val],
                            result=res,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(kind="BYTES_FROM_OBJ", args=[source_val], result=res)
                    )
                return res
            if func_id == "bytearray":
                if any(kw.arg is None for kw in node.keywords):
                    raise NotImplementedError("bytearray does not support **kwargs")
                if len(node.args) > 3:
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    return self._emit_dynamic_call(node, callee, True)
                source_expr = node.args[0] if node.args else None
                encoding_expr = node.args[1] if len(node.args) > 1 else None
                errors_expr = node.args[2] if len(node.args) > 2 else None
                has_encoding = encoding_expr is not None
                has_errors = errors_expr is not None
                for kw in node.keywords:
                    if kw.arg == "source":
                        if source_expr is not None:
                            return self._emit_type_error_value(
                                "bytearray() got multiple values for argument 'source'",
                                "bytearray",
                            )
                        source_expr = kw.value
                    elif kw.arg == "encoding":
                        if has_encoding:
                            return self._emit_type_error_value(
                                "bytearray() got multiple values for argument 'encoding'",
                                "bytearray",
                            )
                        encoding_expr = kw.value
                        has_encoding = True
                    elif kw.arg == "errors":
                        if has_errors:
                            return self._emit_type_error_value(
                                "bytearray() got multiple values for argument 'errors'",
                                "bytearray",
                            )
                        errors_expr = kw.value
                        has_errors = True
                    else:
                        msg = (
                            f"bytearray() got an unexpected keyword argument '{kw.arg}'"
                        )
                        return self._emit_type_error_value(msg, "bytearray")
                if source_expr is None and not has_encoding and not has_errors:
                    arg = MoltValue(self.next_var(), type_hint="bytes")
                    self.emit(MoltOp(kind="CONST_BYTES", args=[b""], result=arg))
                    res = MoltValue(self.next_var(), type_hint="bytearray")
                    self.emit(MoltOp(kind="BYTEARRAY_FROM_OBJ", args=[arg], result=res))
                    return res
                if source_expr is None:
                    source_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=source_val))
                else:
                    source_val = self.visit(source_expr)
                    if source_val is None:
                        raise NotImplementedError("Unsupported bytearray input")
                if has_encoding:
                    if encoding_expr is None:
                        raise NotImplementedError("Unsupported bytearray encoding")
                    encoding_val = self.visit(encoding_expr)
                    if encoding_val is None:
                        raise NotImplementedError("Unsupported bytearray encoding")
                else:
                    encoding_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=encoding_val))
                if has_errors:
                    if errors_expr is None:
                        raise NotImplementedError("Unsupported bytearray errors")
                    errors_val = self.visit(errors_expr)
                    if errors_val is None:
                        raise NotImplementedError("Unsupported bytearray errors")
                else:
                    errors_val = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=errors_val))
                res = MoltValue(self.next_var(), type_hint="bytearray")
                if has_encoding or has_errors:
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FROM_STR",
                            args=[source_val, encoding_val, errors_val],
                            result=res,
                        )
                    )
                else:
                    self.emit(
                        MoltOp(
                            kind="BYTEARRAY_FROM_OBJ",
                            args=[source_val],
                            result=res,
                        )
                    )
                return res
            if func_id == "memoryview":
                if len(node.args) != 1:
                    raise NotImplementedError("memoryview expects 1 argument")
                arg = self.visit(node.args[0])
                res = MoltValue(self.next_var(), type_hint="memoryview")
                self.emit(MoltOp(kind="MEMORYVIEW_NEW", args=[arg], result=res))
                return res
            if func_id in BUILTIN_FUNC_SPECS:
                if func_id == "open":
                    needs_bind = True
                callee = self._emit_builtin_function(func_id)
                res = MoltValue(self.next_var(), type_hint="Any")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(kind="CALL_BIND", args=[callee, callargs], result=res)
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            if target_info is not None:
                target_module = None
                if imported_from == "molt":
                    target_module = MOLT_REEXPORT_FUNCTIONS.get(func_id)
                elif imported_from:
                    normalized = self._normalize_allowlist_module(imported_from)
                    if (
                        normalized in MOLT_DIRECT_CALLS
                        and func_id in MOLT_DIRECT_CALLS[normalized]
                    ):
                        target_module = normalized
                    elif (
                        imported_from in MOLT_DIRECT_CALLS
                        and func_id in MOLT_DIRECT_CALLS[imported_from]
                    ):
                        target_module = imported_from
                if target_module is not None:
                    if needs_bind:
                        callee = self.visit(node.func)
                        if callee is None:
                            raise NotImplementedError("Unsupported call target")
                        res = MoltValue(self.next_var(), type_hint="Any")
                        callargs = self._emit_call_args_builder(node)
                        self.emit(
                            MoltOp(
                                kind="CALL_BIND",
                                args=[callee, callargs],
                                result=res,
                            )
                        )
                        return res
                    args = self._emit_direct_call_args(target_module, func_id, node)
                    res = MoltValue(self.next_var(), type_hint="Any")
                    target_name = (
                        f"{self._sanitize_module_name(target_module)}__{func_id}"
                    )
                    self.emit(
                        MoltOp(kind="CALL", args=[target_name] + args, result=res)
                    )
                    return res
                if imported_from is not None:
                    normalized = self._normalize_allowlist_module(imported_from)
                else:
                    normalized = None
                if imported_from is not None and (
                    imported_from in self.stdlib_allowlist
                    or (normalized is not None and normalized in self.stdlib_allowlist)
                ):
                    callee = self.visit(node.func)
                    if callee is None:
                        raise NotImplementedError("Unsupported call target")
                    res = MoltValue(self.next_var(), type_hint="Any")
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                    return res

            if imported_from is None:
                callee = self.visit(node.func)
                if callee is not None:
                    return self._emit_dynamic_call(node, callee, needs_bind)

            suggestion = self._call_allowlist_suggestion(func_id, imported_from)
            if suggestion:
                alternative = f"use {suggestion}"
            else:
                alternative = (
                    "import from an allowlisted module (see docs/spec/"
                    "0015_STDLIB_COMPATIBILITY_MATRIX.md)"
                )
            detail = (
                "Tier 0 only allows direct calls to allowlisted module-level"
                " functions; rebinding/monkey-patching is not observed"
            )
            if suggestion:
                detail = f"{detail}. warning: allowlisted path is {suggestion}"
            if self.fallback_policy == "bridge":
                self.compat.bridge_unavailable(
                    node,
                    f"call to non-allowlisted function '{func_id}'",
                    impact="high",
                    alternative=alternative,
                    detail=detail,
                )
                callee = self.visit(node.func)
                if callee is None:
                    raise NotImplementedError("Unsupported call target")
                res = MoltValue(self.next_var(), type_hint="Any")
                if needs_bind:
                    callargs = self._emit_call_args_builder(node)
                    self.emit(
                        MoltOp(
                            kind="CALL_BIND",
                            args=[callee, callargs],
                            result=res,
                        )
                    )
                else:
                    args = self._emit_call_args(node.args)
                    self.emit(
                        MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res)
                    )
                return res

            raise self.compat.unsupported(
                node,
                f"call to non-allowlisted function '{func_id}'",
                impact="high",
                alternative=alternative,
                detail=detail,
            )

        callee = self.visit(node.func)
        if callee is None:
            raise NotImplementedError("Unsupported call target")
        return self._emit_dynamic_call(node, callee, needs_bind)

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

    def visit_Slice(self, node: ast.Slice) -> Any:
        if node.lower is None:
            start = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=start))
        else:
            start = self.visit(node.lower)
        if node.upper is None:
            stop = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=stop))
        else:
            stop = self.visit(node.upper)
        if node.step is None:
            step = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=step))
        else:
            step = self.visit(node.step)
        if start is None or stop is None or step is None:
            raise NotImplementedError("Unsupported slice element")
        res = MoltValue(self.next_var(), type_hint="slice")
        self.emit(MoltOp(kind="SLICE_NEW", args=[start, stop, step], result=res))
        return res

    def _emit_attribute_load(
        self,
        node: ast.Attribute,
        obj: MoltValue,
        obj_name: str | None,
        exact_class: str | None,
    ) -> MoltValue:
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
        if class_info:
            getattribute_info, _ = self._resolve_method_info(
                obj.type_hint, "__getattribute__"
            )
            if getattribute_info:
                res = MoltValue(self.next_var())
                self.emit(
                    MoltOp(
                        kind="GETATTR_GENERIC_PTR",
                        args=[obj, node.attr],
                        result=res,
                    )
                )
                return res
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
            hint = None
            if self._hints_enabled():
                hint = class_info.get("field_hints", {}).get(node.attr)
            res = MoltValue(self.next_var(), type_hint=hint or "Unknown")
            self.emit(MoltOp(kind="DATACLASS_GET", args=[obj, idx_val], result=res))
            return res
        method_info = None
        method_class = None
        if class_info:
            method_info, method_class = self._resolve_method_info(
                obj.type_hint, node.attr
            )
        if method_info and method_info["descriptor"] == "function":
            fields = class_info.get("fields", {}) if class_info else {}
            if (
                class_info
                and not class_info.get("dynamic")
                and class_info.get("module") == self.module_name
                and node.attr not in fields
                and not self._instance_attr_mutated(obj.type_hint, node.attr)
            ):
                func_val = method_info["func"]
                if self.current_func_name != "molt_main":
                    class_ref = MoltValue(self.next_var(), type_hint="type")
                    self.emit(MoltOp(kind="TYPE_OF", args=[obj], result=class_ref))
                    func_val = self._emit_class_method_func(class_ref, node.attr)
                class_name = method_class or obj.type_hint
                res = MoltValue(
                    self.next_var(),
                    type_hint=f"BoundMethod:{class_name}:{node.attr}",
                )
                self.emit(
                    MoltOp(
                        kind="BOUND_METHOD_NEW",
                        args=[func_val, obj],
                        result=res,
                    )
                )
                return res
        if (
            method_info
            and method_info["descriptor"] == "property"
            and class_info
            and not class_info.get("dynamic")
        ):
            property_field = method_info.get("property_field")
            if property_field:
                field_map = class_info.get("fields", {})
                if (
                    property_field in field_map
                    and not self._class_attr_is_data_descriptor(
                        obj.type_hint, property_field
                    )
                ):
                    guard = self._loop_guard_for(obj, obj.type_hint, obj_name=obj_name)
                    if guard is None:
                        guard = self._emit_layout_guard(obj, obj.type_hint)
                    return self._emit_guarded_field_get_with_guard(
                        obj,
                        fast_attr=property_field,
                        fallback_attr=node.attr,
                        expected_class=obj.type_hint,
                        guard=guard,
                    )
            getter_symbol = method_info["func"].type_hint.split(":", 1)[1]
            return self._emit_guarded_property_get(
                obj,
                node.attr,
                getter_symbol,
                obj.type_hint,
                method_info["return_hint"],
                obj_name=obj_name,
            )
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
        if expected_class is None:
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_OBJ",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        if self.classes[expected_class].get("dynamic"):
            res = MoltValue(self.next_var())
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
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        if self._class_attr_is_data_descriptor(expected_class, node.attr):
            res = MoltValue(self.next_var())
            self.emit(
                MoltOp(
                    kind="GETATTR_GENERIC_PTR",
                    args=[obj, node.attr],
                    result=res,
                )
            )
            return res
        hint = None
        if self._hints_enabled():
            hint = self.classes[expected_class].get("field_hints", {}).get(node.attr)
        assume_exact = exact_class == expected_class if exact_class else False
        res = self._emit_guarded_getattr(
            obj,
            node.attr,
            expected_class,
            assume_exact=assume_exact,
            obj_name=obj_name,
        )
        if hint is not None:
            res.type_hint = hint
        return res

    def visit_Attribute(self, node: ast.Attribute) -> Any:
        obj = self.visit(node.value)
        if obj is None:
            obj = MoltValue("unknown_obj", type_hint="Unknown")
        obj_name = None
        exact_class = None
        if isinstance(node.value, ast.Name):
            obj_name = node.value.id
            exact_class = self.exact_locals.get(obj_name)
        return self._emit_attribute_load(node, obj, obj_name, exact_class)

    def visit_AnnAssign(self, node: ast.AnnAssign) -> None:
        if not isinstance(node.target, (ast.Name, ast.Attribute)):
            raise NotImplementedError("Only simple annotated assignments are supported")
        hint = None
        if self._hints_enabled():
            hint = self._annotation_to_hint(node.annotation)
            if (
                isinstance(node.target, ast.Name)
                and hint is not None
                and node.target.id not in self.explicit_type_hints
            ):
                self.explicit_type_hints[node.target.id] = hint
        if isinstance(node.target, ast.Name) and self.current_func_name == "molt_main":
            if self.future_annotations:
                ann_dict = self._emit_module_annotations_dict()
                key_val = MoltValue(self.next_var(), type_hint="str")
                self.emit(
                    MoltOp(kind="CONST_STR", args=[node.target.id], result=key_val)
                )
                ann_val = self._emit_annotation_value(node.annotation, stringize=True)
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[ann_dict, key_val, ann_val],
                        result=MoltValue("none"),
                    )
                )
            else:
                exec_map = self._ensure_module_annotation_exec_map()
                exec_id = self.module_annotation_ids.get(id(node))
                if exec_id is None:
                    exec_id = self._annotation_exec_id(is_module=True)
                    self.module_annotation_items.append(
                        (node.target.id, node.annotation, exec_id)
                    )
                self._emit_annotation_exec_mark(exec_map, exec_id)
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
        obj_name = None
        if isinstance(node.target.value, ast.Name):
            class_name = node.target.value.id
            obj_name = class_name
            if class_name in self.classes:
                self._invalidate_loop_guards_for_class(class_name)
        exact_class = None
        if isinstance(node.target.value, ast.Name):
            exact_class = self.exact_locals.get(node.target.value.id)
        class_info = None
        if obj is not None:
            class_info = self.classes.get(obj.type_hint)
        if exact_class is not None:
            self._record_instance_attr_mutation(exact_class, node.target.attr)
        elif obj is not None and obj.type_hint in self.classes:
            self._record_instance_attr_mutation(obj.type_hint, node.target.attr)
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
                    self._emit_guarded_setattr(
                        obj,
                        node.target.attr,
                        value_node,
                        exact_class,
                        obj_name=obj_name,
                        assume_exact=True,
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
                            obj,
                            node.target.attr,
                            value_node,
                            obj.type_hint,
                            obj_name=obj_name,
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

    def _emit_unpack_assign(
        self, target: ast.Tuple | ast.List, value_node: MoltValue | None
    ) -> None:
        if value_node is None:
            raise NotImplementedError("Unsupported unpack assignment value")
        star_index: int | None = None
        for idx, elt in enumerate(target.elts):
            if isinstance(elt, ast.Starred):
                if star_index is not None:
                    raise NotImplementedError(
                        "Multiple starred assignment is not supported"
                    )
                star_index = idx
        seq_val: MoltValue | None = None
        length: MoltValue | None = None

        def emit_unpack_error(
            prefix: str, expected: MoltValue, got: MoltValue | None
        ) -> None:
            parts: list[MoltValue] = []
            head = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[prefix], result=head))
            parts.append(head)
            parts.append(self._emit_str_from_obj(expected))
            if got is not None:
                mid = MoltValue(self.next_var(), type_hint="str")
                self.emit(MoltOp(kind="CONST_STR", args=[", got "], result=mid))
                parts.append(mid)
                parts.append(self._emit_str_from_obj(got))
            tail = MoltValue(self.next_var(), type_hint="str")
            self.emit(MoltOp(kind="CONST_STR", args=[")"], result=tail))
            parts.append(tail)
            msg_val = self._emit_string_join(parts)
            exc_val = self._emit_exception_new("ValueError", msg_val)
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
            self._emit_raise_exit()

        if star_index is None and not self._iterable_is_indexable(value_node):
            expected_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[len(target.elts)], result=expected_val)
            )
            seq_val = MoltValue(self.next_var(), type_hint="list")
            self.emit(MoltOp(kind="LIST_NEW", args=[], result=seq_val))
            iter_obj = self._emit_iter_new(value_node)
            zero = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[0], result=zero))
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            for idx in range(len(target.elts)):
                pair = self._emit_iter_next_checked(iter_obj)
                done = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
                self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
                got_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[idx], result=got_val))
                emit_unpack_error(
                    "not enough values to unpack (expected ",
                    expected_val,
                    got_val,
                )
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                item = MoltValue(self.next_var(), type_hint="Any")
                self.emit(MoltOp(kind="INDEX", args=[pair, zero], result=item))
                self.emit(
                    MoltOp(
                        kind="LIST_APPEND",
                        args=[seq_val, item],
                        result=MoltValue("none"),
                    )
                )
            pair = self._emit_iter_next_checked(iter_obj)
            done = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="INDEX", args=[pair, one], result=done))
            self.emit(MoltOp(kind="IF", args=[done], result=MoltValue("none")))
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            emit_unpack_error(
                "too many values to unpack (expected ", expected_val, None
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        if star_index is not None:
            if seq_val is None:
                seq_val = self._emit_list_from_iter(value_node)
            if length is None:
                length = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[seq_val], result=length))
        if star_index is None:
            if seq_val is None:
                seq_val = self._emit_list_from_iter(value_node)
            if length is None:
                length = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="LEN", args=[seq_val], result=length))
            expected_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[len(target.elts)], result=expected_val)
            )
            too_few = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[length, expected_val], result=too_few))
            self.emit(MoltOp(kind="IF", args=[too_few], result=MoltValue("none")))
            emit_unpack_error(
                "not enough values to unpack (expected ", expected_val, length
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

            too_many = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="LT", args=[expected_val, length], result=too_many))
            self.emit(MoltOp(kind="IF", args=[too_many], result=MoltValue("none")))
            emit_unpack_error(
                "too many values to unpack (expected ", expected_val, None
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

            for idx, elt in enumerate(target.elts):
                idx_val = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
                item_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="INDEX", args=[seq_val, idx_val], result=item_val)
                )
                self._emit_assign_target(elt, item_val, None)
            return

        prefix_len = star_index
        suffix_len = len(target.elts) - star_index - 1
        min_expected = prefix_len + suffix_len
        min_expected_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[min_expected], result=min_expected_val))
        too_few = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="LT", args=[length, min_expected_val], result=too_few))
        self.emit(MoltOp(kind="IF", args=[too_few], result=MoltValue("none")))
        emit_unpack_error(
            "not enough values to unpack (expected at least ",
            min_expected_val,
            length,
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        for idx in range(prefix_len):
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[idx], result=idx_val))
            item_val = MoltValue(self.next_var(), type_hint="Any")
            self.emit(MoltOp(kind="INDEX", args=[seq_val, idx_val], result=item_val))
            self._emit_assign_target(target.elts[idx], item_val, None)

        start_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[prefix_len], result=start_val))
        end_val = length
        if suffix_len:
            suffix_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[suffix_len], result=suffix_val))
            end_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="SUB", args=[length, suffix_val], result=end_val))
        slice_val = MoltValue(self.next_var(), type_hint="list")
        self.emit(
            MoltOp(kind="SLICE", args=[seq_val, start_val, end_val], result=slice_val)
        )
        star_target = cast(ast.Starred, target.elts[star_index]).value
        self._emit_assign_target(star_target, slice_val, None)

        if suffix_len:
            suffix_base = end_val
            for offset in range(suffix_len):
                if offset == 0:
                    idx_val = suffix_base
                else:
                    offset_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(MoltOp(kind="CONST", args=[offset], result=offset_val))
                    idx_val = MoltValue(self.next_var(), type_hint="int")
                    self.emit(
                        MoltOp(
                            kind="ADD", args=[suffix_base, offset_val], result=idx_val
                        )
                    )
                item_val = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(kind="INDEX", args=[seq_val, idx_val], result=item_val)
                )
                self._emit_assign_target(
                    target.elts[star_index + 1 + offset], item_val, None
                )
        return

    def _emit_attribute_store(
        self,
        obj: MoltValue | None,
        obj_expr: ast.AST | None,
        obj_name: str | None,
        exact_class: str | None,
        attr: str,
        value_node: MoltValue,
    ) -> None:
        if obj_expr is not None and isinstance(obj_expr, ast.Name):
            class_name = obj_expr.id
            if class_name in self.classes:
                self._invalidate_loop_guards_for_class(class_name)
        class_info = None
        if obj is not None:
            class_info = self.classes.get(obj.type_hint)
        if exact_class is not None:
            self._record_instance_attr_mutation(exact_class, attr)
        elif obj is not None and obj.type_hint in self.classes:
            self._record_instance_attr_mutation(obj.type_hint, attr)
        if exact_class is not None and obj is not None:
            exact_info = self.classes.get(exact_class)
            if (
                exact_info
                and not exact_info.get("dynamic")
                and not exact_info.get("dataclass")
            ):
                field_map = exact_info.get("fields", {})
                if attr in field_map and not self._class_attr_is_data_descriptor(
                    exact_class, attr
                ):
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        exact_class,
                        obj_name=obj_name,
                        assume_exact=True,
                    )
                    return
        if class_info and class_info.get("dataclass"):
            field_map = class_info["fields"]
            if attr not in field_map:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_OBJ",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
                return
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[field_map[attr]], result=idx_val))
            self.emit(
                MoltOp(
                    kind="DATACLASS_SET",
                    args=[obj, idx_val, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        field_map = class_info.get("fields", {}) if class_info else {}
        if obj is not None and obj.type_hint in self.classes:
            if class_info and class_info.get("dynamic"):
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
            elif attr in field_map:
                if self._class_attr_is_data_descriptor(obj.type_hint, attr):
                    self.emit(
                        MoltOp(
                            kind="SETATTR_GENERIC_PTR",
                            args=[obj, attr, value_node],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    self._emit_guarded_setattr(
                        obj,
                        attr,
                        value_node,
                        obj.type_hint,
                        obj_name=obj_name,
                    )
            else:
                self.emit(
                    MoltOp(
                        kind="SETATTR_GENERIC_PTR",
                        args=[obj, attr, value_node],
                        result=MoltValue("none"),
                    )
                )
        else:
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[obj, attr, value_node],
                    result=MoltValue("none"),
                )
            )

    def _emit_assign_target(
        self,
        target: ast.AST,
        value_node: MoltValue | None,
        source_expr: ast.AST | None,
    ) -> None:
        if isinstance(target, (ast.Tuple, ast.List)):
            self._emit_unpack_assign(target, value_node)
            return
        if value_node is None:
            raise NotImplementedError("Unsupported assignment value")
        if isinstance(target, ast.Attribute):
            obj = self.visit(target.value)
            obj_name = None
            exact_class = None
            if isinstance(target.value, ast.Name):
                obj_name = target.value.id
                exact_class = self.exact_locals.get(obj_name)
            self._emit_attribute_store(
                obj,
                target.value,
                obj_name,
                exact_class,
                target.attr,
                value_node,
            )
            return
        if isinstance(target, ast.Name):
            if (
                self.current_func_name == "molt_main"
                or target.id not in self.global_decls
            ):
                if source_expr is not None:
                    self._update_exact_local(target.id, source_expr)
                    self._propagate_func_type_hint(value_node, source_expr)
            if self.current_func_name != "molt_main" and target.id in self.global_decls:
                self._store_local_value(target.id, value_node)
                return
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
            return
        if isinstance(target, ast.Subscript):
            target_obj = self.visit(target.value)
            if isinstance(target.slice, ast.Slice):
                if target_obj is None:
                    raise NotImplementedError("Unsupported slice assignment target")
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
                    MoltOp(kind="SLICE_NEW", args=[start, end, step], result=slice_obj)
                )
                self.emit(
                    MoltOp(
                        kind="STORE_INDEX",
                        args=[target_obj, slice_obj, value_node],
                        result=MoltValue("none"),
                    )
                )
                return
            index_val = self.visit(target.slice)
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[target_obj, index_val, value_node],
                    result=MoltValue("none"),
                )
            )
            return
        raise NotImplementedError("Unsupported assignment target")

    def visit_Assign(self, node: ast.Assign) -> None:
        value_node = self.visit(node.value)
        for target in node.targets:
            self._emit_assign_target(target, value_node, node.value)
        return None

    def visit_NamedExpr(self, node: ast.NamedExpr) -> Any:
        value_node = self.visit(node.value)
        if value_node is None:
            raise NotImplementedError("Unsupported assignment expression value")
        if not isinstance(node.target, ast.Name):
            raise NotImplementedError("Unsupported assignment expression target")
        self._emit_assign_target(node.target, value_node, node.value)
        return value_node

    def visit_Delete(self, node: ast.Delete) -> None:
        def delete_target(target: ast.AST) -> None:
            if isinstance(target, (ast.Tuple, ast.List)):
                for elt in target.elts:
                    delete_target(elt)
                return
            if isinstance(target, ast.Name):
                name = target.id
                if self.current_func_name == "molt_main":
                    self.locals.pop(name, None)
                    self.globals.pop(name, None)
                    self._emit_module_global_del(name)
                    return
                if name in self.global_decls:
                    self._emit_module_global_del(name)
                    return
                if name in self.nonlocal_decls or name in self.free_vars:
                    _ = self._emit_free_var_load(name)
                    missing = self._emit_missing_value()
                    if not self._emit_free_var_store(name, missing):
                        raise NotImplementedError("nonlocal binding not found")
                    return
                self._box_local(name)
                _ = self._load_local_value(name)
                missing = self._emit_missing_value()
                self._store_local_value(name, missing)
                return
            if isinstance(target, ast.Attribute):
                obj = self.visit(target.value)
                if obj is None:
                    raise NotImplementedError("del expects attribute owner")
                exact_class = None
                if isinstance(target.value, ast.Name):
                    exact_class = self.exact_locals.get(target.value.id)
                if exact_class is not None:
                    self._record_instance_attr_mutation(exact_class, target.attr)
                elif obj.type_hint in self.classes:
                    self._record_instance_attr_mutation(obj.type_hint, target.attr)
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
                return
            if isinstance(target, ast.Subscript):
                target_obj = self.visit(target.value)
                if target_obj is None:
                    raise NotImplementedError("del expects subscript owner")
                if isinstance(target.slice, ast.Slice):
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
                            kind="DEL_INDEX",
                            args=[target_obj, slice_obj],
                            result=MoltValue("none"),
                        )
                    )
                    return
                index_val = self.visit(target.slice)
                self.emit(
                    MoltOp(
                        kind="DEL_INDEX",
                        args=[target_obj, index_val],
                        result=MoltValue("none"),
                    )
                )
                return
            raise NotImplementedError(
                "del only supports name, attribute, or subscript deletion"
            )

        for target in node.targets:
            delete_target(target)
        return None

    def _augassign_op_kind(self, op: ast.operator) -> str:
        if isinstance(op, ast.Add):
            return "INPLACE_ADD"
        if isinstance(op, ast.Sub):
            return "INPLACE_SUB"
        if isinstance(op, ast.Mult):
            return "INPLACE_MUL"
        if isinstance(op, ast.Div):
            return "DIV"
        if isinstance(op, ast.FloorDiv):
            return "FLOORDIV"
        if isinstance(op, ast.Mod):
            return "MOD"
        if isinstance(op, ast.Pow):
            return "POW"
        if isinstance(op, ast.BitOr):
            return "INPLACE_BIT_OR"
        if isinstance(op, ast.BitAnd):
            return "INPLACE_BIT_AND"
        if isinstance(op, ast.BitXor):
            return "INPLACE_BIT_XOR"
        if isinstance(op, ast.LShift):
            return "LSHIFT"
        if isinstance(op, ast.RShift):
            return "RSHIFT"
        if isinstance(op, ast.MatMult):
            return "MATMUL"
        raise NotImplementedError("Unsupported augmented assignment operator")

    def visit_AugAssign(self, node: ast.AugAssign) -> None:
        op_kind = self._augassign_op_kind(node.op)
        may_yield = self._expr_may_yield(node.value)
        if isinstance(node.target, ast.Name):
            self.exact_locals.pop(node.target.id, None)
            load_node = ast.Name(id=node.target.id, ctx=ast.Load())
            if may_yield and self.is_async() and node.target.id in self.async_locals:
                value_node = self.visit(node.value)
                current = self._load_local_value(node.target.id)
            else:
                current = self.visit(load_node)
                value_node = self.visit(node.value)
            if current is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            if value_node is None:
                raise NotImplementedError("Unsupported augmented assignment value")
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
            obj = self.visit(node.target.value)
            if obj is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            obj_name = None
            exact_class = None
            if isinstance(node.target.value, ast.Name):
                obj_name = node.target.value.id
                exact_class = self.exact_locals.get(obj_name)
            current = self._emit_attribute_load(node.target, obj, obj_name, exact_class)
            if self.is_async() and may_yield:
                obj_slot = self._spill_async_value(
                    obj, f"__augattr_obj_{len(self.async_locals)}"
                )
                current_slot = self._spill_async_value(
                    current, f"__augattr_cur_{len(self.async_locals)}"
                )
                value_node = self.visit(node.value)
                obj = self._reload_async_value(obj_slot, obj.type_hint)
                current = self._reload_async_value(current_slot, current.type_hint)
            else:
                value_node = self.visit(node.value)
            if value_node is None:
                raise NotImplementedError("Unsupported augmented assignment value")
            if current is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            res = MoltValue(self.next_var(), type_hint=current.type_hint)
            self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
            self._emit_attribute_store(
                obj,
                node.target.value,
                obj_name,
                exact_class,
                node.target.attr,
                res,
            )
            return None
        if isinstance(node.target, ast.Subscript):
            target_obj = self.visit(node.target.value)
            if target_obj is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            if isinstance(node.target.slice, ast.Slice):
                raise NotImplementedError("Slice augmented assignment is not supported")
            index_val = self.visit(node.target.slice)
            if index_val is None:
                raise NotImplementedError("Unsupported augmented assignment target")
            current = MoltValue(self.next_var(), type_hint="Any")
            self.emit(
                MoltOp(
                    kind="INDEX",
                    args=[target_obj, index_val],
                    result=current,
                )
            )
            if self.is_async() and may_yield:
                obj_slot = self._spill_async_value(
                    target_obj, f"__augsub_obj_{len(self.async_locals)}"
                )
                idx_slot = self._spill_async_value(
                    index_val, f"__augsub_idx_{len(self.async_locals)}"
                )
                cur_slot = self._spill_async_value(
                    current, f"__augsub_cur_{len(self.async_locals)}"
                )
                value_node = self.visit(node.value)
                target_obj = self._reload_async_value(obj_slot, target_obj.type_hint)
                index_val = self._reload_async_value(idx_slot, index_val.type_hint)
                current = self._reload_async_value(cur_slot, current.type_hint)
            else:
                value_node = self.visit(node.value)
            if value_node is None:
                raise NotImplementedError("Unsupported augmented assignment value")
            res = MoltValue(self.next_var(), type_hint=current.type_hint)
            self.emit(MoltOp(kind=op_kind, args=[current, value_node], result=res))
            self.emit(
                MoltOp(
                    kind="STORE_INDEX",
                    args=[target_obj, index_val, res],
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
        if operand is None:
            raise NotImplementedError("Unsupported unary operand")
        if isinstance(node.op, ast.UAdd):
            return operand
        if isinstance(node.op, ast.USub):
            if operand.type_hint == "float":
                zero = MoltValue(self.next_var(), type_hint="float")
                self.emit(MoltOp(kind="CONST_FLOAT", args=[0.0], result=zero))
                res = MoltValue(self.next_var(), type_hint="float")
            else:
                zero = MoltValue(self.next_var(), type_hint="int")
                self.emit(MoltOp(kind="CONST", args=[0], result=zero))
                res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="SUB", args=[zero, operand], result=res))
            return res
        if isinstance(node.op, ast.Not):
            return self._emit_not(operand)
        if isinstance(node.op, ast.Invert):
            res = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="INVERT", args=[operand], result=res))
            return res
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
        if self._is_type_checking_test(node.test):
            if node.orelse:
                if not self.is_async():
                    assigned = self._collect_assigned_names(node.orelse)
                    for name in sorted(assigned):
                        self._box_local(name)
                self._visit_block(node.orelse)
            return None
        cond = self.visit(node.test)
        if not self.is_async():
            assigned = self._collect_assigned_names(node.body + node.orelse)
            for name in sorted(assigned):
                self._box_local(name)
        self.emit(MoltOp(kind="IF", args=[cond], result=MoltValue("none")))
        self.control_flow_depth += 1
        try:
            self._visit_block(node.body)
            if node.orelse:
                self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
                self._visit_block(node.orelse)
        finally:
            self.control_flow_depth -= 1
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return None

    def visit_With(self, node: ast.With) -> None:
        if len(node.items) != 1:
            nested = ast.With(
                items=node.items[1:],
                body=node.body,
                type_comment=None,
            )
            ast.copy_location(nested, node)
            outer = ast.With(
                items=[node.items[0]],
                body=[nested],
                type_comment=node.type_comment,
            )
            ast.copy_location(outer, node)
            return self.visit_With(outer)

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

        ctx_name = f"__molt_with_ctx_{self.next_label()}"
        self._store_local_value(ctx_name, ctx_val)
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
        self.try_scopes.append(
            TryScope(
                ctx_mark=ctx_mark,
                finalbody=None,
                ctx_mark_offset=ctx_mark_offset,
            )
        )
        ctx_ref = self._load_local_value(ctx_name) or ctx_val
        enter_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CONTEXT_ENTER", args=[ctx_ref], result=enter_val))
        if item.optional_vars is not None:
            self._emit_assign_target(item.optional_vars, enter_val, None)
        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_end_label = self.next_label()
        self.try_end_labels.append(try_end_label)
        self.emit(MoltOp(kind="TRY_START", args=[], result=MoltValue("none")))
        self.context_depth += 1
        self.control_flow_depth += 1
        try:
            self._visit_block(node.body)
        finally:
            self.control_flow_depth -= 1
            self.context_depth -= 1
        self.emit(MoltOp(kind="LABEL", args=[try_end_label], result=MoltValue("none")))
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
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        exit_res = MoltValue(self.next_var(), type_hint="Any")
        ctx_ref = self._load_local_value(ctx_name) or ctx_val
        self.emit(MoltOp(kind="CONTEXT_EXIT", args=[ctx_ref, exc_val], result=exit_res))
        self._emit_raise_if_pending()
        not_res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[exit_res], result=not_res))
        is_truthy = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[not_res], result=is_truthy))
        self.emit(MoltOp(kind="IF", args=[is_truthy], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        exit_ok = MoltValue(self.next_var(), type_hint="Any")
        ctx_ref = self._load_local_value(ctx_name) or ctx_val
        self.emit(MoltOp(kind="CONTEXT_EXIT", args=[ctx_ref, none_val], result=exit_ok))
        self._emit_raise_if_pending()
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.try_scopes.pop()
        self.try_suppress_depth = prior_suppress
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True)
        return None

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        if not self.is_async():
            raise NotImplementedError("async with is only supported in async functions")
        if len(node.items) != 1:
            nested = ast.AsyncWith(
                items=node.items[1:],
                body=node.body,
                type_comment=None,
            )
            ast.copy_location(nested, node)
            outer = ast.AsyncWith(
                items=[node.items[0]],
                body=[nested],
                type_comment=node.type_comment,
            )
            ast.copy_location(outer, node)
            return self.visit_AsyncWith(outer)

        item = node.items[0]
        ctx_val = self.visit(item.context_expr)
        if ctx_val is None:
            self._bridge_fallback(
                node,
                "async with",
                impact="high",
                alternative="use contextlib.nullcontext for now",
                detail="context expression did not lower",
            )
            return None

        ctx_slot = self._async_local_offset(
            f"__async_with_ctx_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", ctx_slot, ctx_val],
                result=MoltValue("none"),
            )
        )

        aenter_fn = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_SPECIAL_OBJ",
                args=[ctx_val, "__aenter__"],
                result=aenter_fn,
            )
        )
        aenter_call = self._emit_call_bound_or_func(aenter_fn, [])
        self._emit_raise_if_pending()
        enter_val = self._emit_await_value(aenter_call)
        if item.optional_vars is not None:
            self._emit_assign_target(item.optional_vars, enter_val, None)

        self.emit(MoltOp(kind="EXCEPTION_PUSH", args=[], result=MoltValue("none")))
        try_end_label = self.next_label()
        self.try_end_labels.append(try_end_label)
        self.emit(MoltOp(kind="TRY_START", args=[], result=MoltValue("none")))
        self.control_flow_depth += 1
        try:
            self._visit_block(node.body)
        finally:
            self.control_flow_depth -= 1
        self.emit(MoltOp(kind="LABEL", args=[try_end_label], result=MoltValue("none")))
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

        ctx_reload = MoltValue(self.next_var(), type_hint=ctx_val.type_hint)
        self.emit(
            MoltOp(kind="LOAD_CLOSURE", args=["self", ctx_slot], result=ctx_reload)
        )
        aexit_fn = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(
                kind="GETATTR_SPECIAL_OBJ",
                args=[ctx_reload, "__aexit__"],
                result=aexit_fn,
            )
        )

        self.emit(MoltOp(kind="IF", args=[pending], result=MoltValue("none")))
        exc_slot = self._async_local_offset(
            f"__async_with_exc_{len(self.async_locals)}"
        )
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", exc_slot, exc_val],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        exc_type = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="TYPE_OF", args=[exc_val], result=exc_type))
        tb_val = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=tb_val))
        aexit_call = self._emit_call_bound_or_func(
            aexit_fn, [exc_type, exc_val, tb_val]
        )
        self._emit_raise_if_pending()
        aexit_res = self._emit_await_value(aexit_call, raise_pending=False)
        exc_after = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
        none_after = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
        is_none_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after))
        pending_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none_after], result=pending_after))
        self.emit(MoltOp(kind="IF", args=[pending_after], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        not_res = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[aexit_res], result=not_res))
        is_truthy = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[not_res], result=is_truthy))
        self.emit(MoltOp(kind="IF", args=[is_truthy], result=MoltValue("none")))
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        exc_reload = MoltValue(self.next_var(), type_hint="exception")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", exc_slot],
                result=exc_reload,
            )
        )
        self.emit(MoltOp(kind="RAISE", args=[exc_reload], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        aexit_call = self._emit_call_bound_or_func(
            aexit_fn, [none_val, none_val, none_val]
        )
        self._emit_raise_if_pending()
        self._emit_await_value(aexit_call, raise_pending=False)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        self.try_suppress_depth = prior_suppress
        self.emit(MoltOp(kind="EXCEPTION_POP", args=[], result=MoltValue("none")))
        self._emit_raise_if_pending(emit_exit=True)
        return None

    def visit_For(self, node: ast.For) -> None:
        break_name = None
        if node.orelse:
            while True:
                candidate = f"__molt_for_break_{self.loop_break_counter}"
                self.loop_break_counter += 1
                if (
                    candidate not in self.locals
                    and candidate not in self.globals
                    and candidate not in self.boxed_locals
                ):
                    break_name = candidate
                    break
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            self._store_local_value(break_name, break_init)
            if not self.is_async():
                self._box_local(break_name)
        matmul_match = (
            self._match_matmul_loop(node) if isinstance(node.target, ast.Name) else None
        )
        if matmul_match is not None:
            out_name, a_name, b_name = matmul_match
            a_val = self.locals.get(a_name) or self.globals.get(a_name)
            b_val = self.locals.get(b_name) or self.globals.get(b_name)
            if a_val is None or b_val is None:
                raise NotImplementedError("Matmul operands must be simple locals")
            a_hint = self.boxed_local_hints.get(a_name, a_val.type_hint)
            b_hint = self.boxed_local_hints.get(b_name, b_val.type_hint)
            if a_hint == "buffer2d" and b_hint == "buffer2d":
                a_arg = self._load_local_value(a_name) or a_val
                b_arg = self._load_local_value(b_name) or b_val
                res = MoltValue(self.next_var(), type_hint="buffer2d")
                self.emit(
                    MoltOp(kind="BUFFER2D_MATMUL", args=[a_arg, b_arg], result=res)
                )
                self._store_local_value(out_name, res)
                if break_name is not None:
                    self._emit_loop_orelse(break_name, node.orelse)
                return None
        target_names = self._collect_target_names(node.target)
        if not target_names:
            raise NotImplementedError("Only name/tuple/list for targets are supported")
        for name in target_names:
            self.exact_locals.pop(name, None)
        assigned = self._collect_assigned_names(node.body)
        assigned.update(target_names)
        for name in sorted(assigned):
            if not self.is_async():
                self._box_local(name)
        reduction = None
        if not self.is_async() and isinstance(node.target, ast.Name):
            reduction = self._match_indexed_vector_reduction_loop(node)
            if reduction is None:
                reduction = self._match_indexed_vector_minmax_loop(node)
            if reduction is None:
                reduction = self._match_iter_vector_reduction_loop(node)
            if reduction is None:
                reduction = self._match_iter_vector_minmax_loop(node)
        if reduction is not None:
            acc_name, seq_name, kind, start_expr = reduction
            if seq_name in assigned:
                reduction = None
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
                        if range_args is not None:
                            start, stop, step = range_args
                            self._emit_range_loop(
                                node,
                                start,
                                stop,
                                step,
                                loop_break_flag=break_name,
                            )
                        else:
                            iterable = self._load_local_value(seq_name) or seq_val
                            self._emit_for_loop(
                                node, iterable, loop_break_flag=break_name
                            )
                        self.emit(
                            MoltOp(kind="END_IF", args=[], result=MoltValue("none"))
                        )
                        if break_name is not None:
                            self._emit_loop_orelse(break_name, node.orelse)
                        return None
        range_args = self._parse_range_call(node.iter)
        if range_args is not None:
            start, stop, step = range_args
            self._emit_range_loop(node, start, stop, step, loop_break_flag=break_name)
            if break_name is not None:
                self._emit_loop_orelse(break_name, node.orelse)
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
                self._emit_for_loop(node, iterable, loop_break_flag=break_name)
                self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
                if break_name is not None:
                    self._emit_loop_orelse(break_name, node.orelse)
                return None

        self._emit_for_loop(node, iterable, loop_break_flag=break_name)
        if break_name is not None:
            self._emit_loop_orelse(break_name, node.orelse)
        return None

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        if not self.is_async():
            raise NotImplementedError("async for is only supported in async functions")
        iterable = self.visit(node.iter)
        if iterable is None:
            raise NotImplementedError("Unsupported iterable in async for loop")
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
        break_slot = None
        if node.orelse:
            break_slot = self._async_local_offset(
                f"__async_for_break_{len(self.async_locals)}"
            )
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", break_slot, break_init],
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
        self._emit_assign_target(node.target, item_val, None)
        guard_map = self._emit_hoisted_loop_guards(node.body)
        self._visit_loop_body(node.body, guard_map, loop_break_flag=break_slot)
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))
        if node.orelse:
            break_val = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", break_slot],
                    result=break_val,
                )
            )
            should_run = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="NOT", args=[break_val], result=should_run))
            self.emit(MoltOp(kind="IF", args=[should_run], result=MoltValue("none")))
            self._visit_block(node.orelse)
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        return None

    def visit_While(self, node: ast.While) -> None:
        break_name = None
        if node.orelse:
            while True:
                candidate = f"__molt_while_break_{self.loop_break_counter}"
                self.loop_break_counter += 1
                if (
                    candidate not in self.locals
                    and candidate not in self.globals
                    and candidate not in self.boxed_locals
                ):
                    break_name = candidate
                    break
            break_init = MoltValue(self.next_var(), type_hint="bool")
            self.emit(MoltOp(kind="CONST_BOOL", args=[False], result=break_init))
            self._store_local_value(break_name, break_init)
            if not self.is_async():
                self._box_local(break_name)
        counted = None if break_name is not None else self._match_counted_while(node)
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
            const_inc = self._match_counted_while_const_increment(body)
            if const_inc is not None:
                acc_name, delta = const_inc
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
                    if start_const < bound:
                        span = bound - start_const
                    else:
                        span = 0
                    final_val = acc_const + span * delta
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
        assigned |= self._collect_namedexpr_names(node.test)
        for name in sorted(assigned):
            if not self.is_async():
                self._box_local(name)
        guard_map = self._emit_hoisted_loop_guards(node.body)

        def emit_loop_body() -> None:
            self.emit(MoltOp(kind="LOOP_START", args=[], result=MoltValue("none")))
            cond = self.visit(node.test)
            self.emit(
                MoltOp(
                    kind="LOOP_BREAK_IF_FALSE",
                    args=[cond],
                    result=MoltValue("none"),
                )
            )
            self._visit_loop_body(node.body, None, loop_break_flag=break_name)
            self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="LOOP_END", args=[], result=MoltValue("none")))

        if guard_map:
            guard_cond = self._emit_guard_map_condition(guard_map)
            self.emit(MoltOp(kind="IF", args=[guard_cond], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, True)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self._push_loop_guard_assumptions(guard_map, False)
            emit_loop_body()
            self._pop_loop_guard_assumptions()
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            if break_name is not None:
                self._emit_loop_orelse(break_name, node.orelse)
            return None

        emit_loop_body()
        if break_name is not None:
            self._emit_loop_orelse(break_name, node.orelse)
        return None

    def _visit_block(self, body: list[ast.stmt]) -> None:
        prior = self.block_terminated
        self.block_terminated = False
        for stmt in body:
            self.visit(stmt)
            if self.block_terminated:
                break
            clear_handlers = (
                self.current_func_name == "molt_main" and not self.try_end_labels
            )
            self._emit_raise_if_pending(emit_exit=True, clear_handlers=clear_handlers)
        self.block_terminated = prior

    def _visit_loop_body(
        self,
        body: list[ast.stmt],
        prefill: dict[str, tuple[str, MoltValue]] | None = None,
        loop_break_flag: int | str | None = None,
    ) -> None:
        if not self.is_async():
            guard_map = dict(prefill) if prefill else {}
            self.loop_layout_guards.append(guard_map)
        self.loop_break_flags.append(loop_break_flag)
        self.loop_try_depths.append(len(self.try_scopes))
        try:
            self.control_flow_depth += 1
            try:
                self._visit_block(body)
            finally:
                self.control_flow_depth -= 1
        finally:
            self.loop_break_flags.pop()
            self.loop_try_depths.pop()
            if not self.is_async():
                self.loop_layout_guards.pop()

    def _emit_guarded_body(
        self, body: list[ast.stmt], baseline_exc: ActiveException | None
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
        baseline_val = self._active_exception_value(baseline_exc)
        is_same = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_after, baseline_val], result=is_same))
        continue_guard = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="OR", args=[is_none, is_same], result=continue_guard))
        self.emit(MoltOp(kind="IF", args=[continue_guard], result=MoltValue("none")))
        self._emit_guarded_body(remaining, baseline_exc)
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

    def _emit_finalbody(
        self, finalbody: list[ast.stmt], baseline_exc: ActiveException | None
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

    def _emit_raise_if_pending(
        self, *, emit_exit: bool = False, clear_handlers: bool = False
    ) -> None:
        exc_after = MoltValue(self.next_var(), type_hint="exception")
        self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
        none_after = MoltValue(self.next_var(), type_hint="None")
        self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
        is_none_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after))
        pending_after = MoltValue(self.next_var(), type_hint="bool")
        self.emit(MoltOp(kind="NOT", args=[is_none_after], result=pending_after))
        self.emit(MoltOp(kind="IF", args=[pending_after], result=MoltValue("none")))
        if clear_handlers:
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_CLEAR",
                    args=[],
                    result=MoltValue("none"),
                )
            )
        if self.in_generator and not self.async_context:
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

    def _emit_call_bound_or_func(
        self, callee: MoltValue, args: list[MoltValue]
    ) -> MoltValue:
        # Use CALL_FUNC to centralize bound-method handling and keep async IR linear.
        res = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="CALL_FUNC", args=[callee] + args, result=res))
        return res

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
        if not self.is_async():
            assigned = self._collect_assigned_names([node])
            for name in sorted(assigned):
                self._box_local(name)
        prior_terminated = self.block_terminated
        self.block_terminated = False
        self.control_flow_depth += 1

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
        self._visit_block(node.body)
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
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
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
            exc_entry = ActiveException(value=exc_val, slot=exc_slot_offset)
            self.active_exceptions.append(exc_entry)
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[exc_val],
                    result=MoltValue("none"),
                )
            )
            self._emit_guarded_body(handler.body, exc_entry)
            self.active_exceptions.pop()
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            if len(handlers) > 1:
                emit_handlers(handlers[1:])
            else:
                self.emit(
                    MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none"))
                )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))

        if node.handlers:
            emit_handlers(node.handlers)

        if node.finalbody:
            final_exc = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=final_exc))
            final_slot = None
            if self.is_async():
                final_slot = self._async_local_offset(
                    f"__final_exc_{len(self.async_locals)}"
                )
                self.emit(
                    MoltOp(
                        kind="STORE_CLOSURE",
                        args=["self", final_slot, final_exc],
                        result=MoltValue("none"),
                    )
                )
            final_entry = ActiveException(value=final_exc, slot=final_slot)
            self.active_exceptions.append(final_entry)
            self.emit(
                MoltOp(
                    kind="EXCEPTION_CONTEXT_SET",
                    args=[final_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            self._emit_finalbody(node.finalbody, final_entry)
            exc_after = MoltValue(self.next_var(), type_hint="exception")
            self.emit(MoltOp(kind="EXCEPTION_LAST", args=[], result=exc_after))
            none_after = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_after))
            is_none_after = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(kind="IS", args=[exc_after, none_after], result=is_none_after)
            )
            self.emit(MoltOp(kind="IF", args=[is_none_after], result=MoltValue("none")))
            restored_exc = self._active_exception_value(final_entry)
            is_restore_none = MoltValue(self.next_var(), type_hint="bool")
            self.emit(
                MoltOp(
                    kind="IS", args=[restored_exc, none_after], result=is_restore_none
                )
            )
            self.emit(
                MoltOp(kind="IF", args=[is_restore_none], result=MoltValue("none"))
            )
            self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
            self.emit(
                MoltOp(
                    kind="EXCEPTION_SET_LAST",
                    args=[restored_exc],
                    result=MoltValue("none"),
                )
            )
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
            self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
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
        self.control_flow_depth -= 1
        self.block_terminated = prior_terminated
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
        self.block_terminated = True
        clear_handlers = (
            self.current_func_name == "molt_main" and not self.try_end_labels
        )

        def emit_exception_value(
            expr: ast.expr, *, allow_none: bool, context: str
        ) -> MoltValue | None:
            if allow_none and isinstance(expr, ast.Constant) and expr.value is None:
                none_val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_val))
                return none_val
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
                if clear_handlers:
                    self.emit(
                        MoltOp(
                            kind="EXCEPTION_STACK_CLEAR",
                            args=[],
                            result=MoltValue("none"),
                        )
                    )
                exc_val = self._active_exception_value(self.active_exceptions[-1])
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
            if clear_handlers:
                self.emit(
                    MoltOp(
                        kind="EXCEPTION_STACK_CLEAR",
                        args=[],
                        result=MoltValue("none"),
                    )
                )
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
        if clear_handlers:
            self.emit(
                MoltOp(
                    kind="EXCEPTION_STACK_CLEAR",
                    args=[],
                    result=MoltValue("none"),
                )
            )
        if self.active_exceptions:
            context_val = self._active_exception_value(self.active_exceptions[-1])
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

    def _emit_loop_unwind(self) -> None:
        if not self.loop_try_depths:
            return
        max_scopes = len(self.try_scopes)
        loop_depth = self.loop_try_depths[-1]
        if loop_depth >= max_scopes:
            return
        none_exc = None
        if self.context_depth > 0:
            none_exc = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
        skip_finalbody = self.return_unwind_depth
        for scope in reversed(self.try_scopes[loop_depth:max_scopes]):
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
                if skip_finalbody > 0:
                    skip_finalbody -= 1
                else:
                    prior_active = self.active_exceptions[:]
                    self.active_exceptions.clear()
                    self._emit_finalbody(scope.finalbody, None)
                    self.active_exceptions = prior_active

    def visit_Break(self, node: ast.Break) -> None:
        del node
        if self.loop_break_flags:
            break_slot = self.loop_break_flags[-1]
            if break_slot is not None:
                break_val = MoltValue(self.next_var(), type_hint="bool")
                self.emit(MoltOp(kind="CONST_BOOL", args=[True], result=break_val))
                if isinstance(break_slot, int):
                    self.emit(
                        MoltOp(
                            kind="STORE_CLOSURE",
                            args=["self", break_slot, break_val],
                            result=MoltValue("none"),
                        )
                    )
                else:
                    self._store_local_value(break_slot, break_val)
        self._emit_loop_unwind()
        self.emit(MoltOp(kind="LOOP_BREAK", args=[], result=MoltValue("none")))
        self.block_terminated = True
        return None

    def visit_Continue(self, node: ast.Continue) -> None:
        del node
        self._emit_loop_unwind()
        if self.async_index_loop_stack:
            idx_slot = self.async_index_loop_stack[-1]
            idx_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(
                    kind="LOAD_CLOSURE",
                    args=["self", idx_slot],
                    result=idx_val,
                )
            )
            one = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="CONST", args=[1], result=one))
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx_val, one], result=next_idx))
            self.emit(
                MoltOp(
                    kind="STORE_CLOSURE",
                    args=["self", idx_slot, next_idx],
                    result=MoltValue("none"),
                )
            )
        elif self.range_loop_stack:
            idx, step = self.range_loop_stack[-1]
            next_idx = MoltValue(self.next_var(), type_hint="int")
            self.emit(MoltOp(kind="ADD", args=[idx, step], result=next_idx))
            self.emit(MoltOp(kind="LOOP_INDEX_NEXT", args=[next_idx], result=next_idx))
        self.emit(MoltOp(kind="LOOP_CONTINUE", args=[], result=MoltValue("none")))
        self.block_terminated = True
        return None

    def visit_Return(self, node: ast.Return) -> None:
        self.block_terminated = True
        if self.in_generator:
            val = self.visit(node.value) if node.value is not None else None
            if val is None:
                val = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
            if self.return_unwind_depth == 0:
                self._emit_raise_if_pending(emit_exit=True)
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
            none_exc = None
            if self.try_scopes:
                if self.context_depth > 0:
                    none_exc = MoltValue(self.next_var(), type_hint="None")
                    self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
                skip_finalbody = self.return_unwind_depth
                for scope in reversed(self.try_scopes):
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
                        if skip_finalbody > 0:
                            skip_finalbody -= 1
                        else:
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
        if self.return_unwind_depth == 0:
            self._emit_raise_if_pending(emit_exit=True)
        self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        none_exc = None
        if self.try_scopes:
            if self.context_depth > 0:
                none_exc = MoltValue(self.next_var(), type_hint="None")
                self.emit(MoltOp(kind="CONST_NONE", args=[], result=none_exc))
            skip_finalbody = self.return_unwind_depth
            for scope in reversed(self.try_scopes):
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
                    if skip_finalbody > 0:
                        skip_finalbody -= 1
                    else:
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
        if self.current_func_name == "molt_main":
            self.module_global_mutations.update(self._collect_global_decls(node.body))
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
                params = self._function_param_names(node.args)
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
        if self._function_contains_yield(node):
            if self._async_generator_contains_yield_from(node):
                raise SyntaxError("'yield from' inside async function")
            if self._async_generator_contains_return_value(node):
                raise SyntaxError("'return' with value in async generator")
            func_name = node.name
            qualname = self._qualname_for_def(func_name)
            func_symbol = self._function_symbol(func_name)
            self._record_func_default_specs(func_symbol, node.args)
            poll_func_name = f"{func_symbol}_poll"
            prev_func = self.current_func_name
            has_return = self._function_contains_return(node)
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                node.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(node.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if node.args.vararg is not None:
                arg_nodes.append(node.args.vararg)
            arg_nodes.extend(kwonly)
            if node.args.kwarg is not None:
                arg_nodes.append(node.args.kwarg)

            free_vars: list[str] = []
            free_var_hints: dict[str, str] = {}
            closure_val: MoltValue | None = None
            has_closure = False
            if self.current_func_name != "molt_main":
                free_vars = self._collect_free_vars(node)
                if free_vars:
                    self.unbound_check_names.update(free_vars)
                    for name in free_vars:
                        self._box_local(name)
                    for name in free_vars:
                        hint = self.boxed_local_hints.get(name)
                        if hint is None:
                            value = self.locals.get(name)
                            if value is not None and value.type_hint:
                                hint = value.type_hint
                        free_var_hints[name] = hint or "Any"
                    closure_items = [self.boxed_locals[name] for name in free_vars]
                    closure_val = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                    )
                    has_closure = True

            func_kind = "AsyncGenClosureFunc" if has_closure else "AsyncGenFunc"
            self.globals[func_name] = MoltValue(
                func_name, type_hint=f"{func_kind}:{poll_func_name}:0"
            )

            prev_state = self._capture_function_state()
            self.start_function(
                poll_func_name,
                params=["self"],
                type_facts_name=func_name,
                needs_return_slot=has_return,
            )
            self.async_context = True
            self.global_decls = self._collect_global_decls(node.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
            assigned = self._collect_assigned_names(node.body)
            self.del_targets = self._collect_deleted_names(node.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            self.in_generator = True
            if has_closure:
                self.async_closure_offset = GEN_CONTROL_SIZE
                self.async_locals_base = GEN_CONTROL_SIZE + 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            else:
                self.async_locals_base = GEN_CONTROL_SIZE
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                if self._hints_enabled():
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
                    if hint is not None:
                        self.async_local_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._push_qualname(func_name, True)
            try:
                for item in node.body:
                    self.visit(item)
                    if isinstance(item, (ast.Return, ast.Raise)):
                        break
            finally:
                self._pop_qualname()
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
            self._spill_async_temporaries()
            closure_size = self.async_locals_base + len(self.async_locals) * 8
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)

            func_hint = f"{func_kind}:{poll_func_name}:{closure_size}"
            func_val = MoltValue(self.next_var(), type_hint=func_hint)
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[func_symbol, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[func_symbol, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and self._signature_contains_yield(
                decorators=node.decorator_list,
                args=node.args,
                returns=node.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            self._emit_function_metadata(
                func_val,
                name=func_name,
                qualname=qualname,
                trace_lineno=node.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=node.args.defaults,
                kw_default_exprs=node.args.kw_defaults,
                docstring=ast.get_docstring(node),
                is_async_generator=True,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, node)
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self._store_local_value(func_name, func_val)
            self._emit_module_attr_set(func_name, func_val)

            prev_func = self.current_func_name
            prev_state = self._capture_function_state()
            func_params = params
            if has_closure:
                func_params = [_MOLT_CLOSURE_PARAM] + params
            self.start_function(
                func_symbol,
                params=func_params,
                type_facts_name=func_name,
            )
            if has_closure:
                self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                    _MOLT_CLOSURE_PARAM, type_hint="tuple"
                )
            for idx, arg in enumerate(arg_nodes):
                hint = None
                if idx == 0 and arg.arg == "self":
                    hint = None
                if self._hints_enabled():
                    explicit = self.explicit_type_hints.get(arg.arg)
                    if explicit is None:
                        explicit = self._annotation_to_hint(arg.annotation)
                        if explicit is not None:
                            self.explicit_type_hints[arg.arg] = explicit
                    if explicit is not None:
                        hint = explicit
                    elif hint is None:
                        hint = "Any"
                value = MoltValue(arg.arg, type_hint=hint or "Unknown")
                if hint is not None:
                    self._apply_hint_to_value(arg.arg, value, hint)
                self.locals[arg.arg] = value
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(self.locals[arg.arg], hint)
            args = [self.locals[arg.arg] for arg in arg_nodes]
            if has_closure:
                args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
            gen_val = MoltValue(self.next_var(), type_hint="generator")
            self.emit(
                MoltOp(
                    kind="ALLOC_TASK",
                    args=[poll_func_name, closure_size] + args,
                    result=gen_val,
                    metadata={"task_kind": "generator"},
                )
            )
            res = MoltValue(self.next_var(), type_hint="async_generator")
            self.emit(MoltOp(kind="ASYNCGEN_NEW", args=[gen_val], result=res))
            self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            if node.decorator_list:
                decorated = func_val
                for deco in reversed(node.decorator_list):
                    decorator_val = self.visit(deco)
                    if decorator_val is None:
                        raise NotImplementedError("Unsupported decorator")
                    res_val = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[decorator_val, decorated],
                            result=res_val,
                        )
                    )
                    decorated = res_val
                func_val = decorated
                if self.current_func_name == "molt_main":
                    self.globals[func_name] = func_val
                else:
                    self._store_local_value(func_name, func_val)
                self._emit_module_attr_set(func_name, func_val)
            return None
        func_name = node.name
        qualname = self._qualname_for_def(func_name)
        func_symbol = self._function_symbol(func_name)
        self._record_func_default_specs(func_symbol, node.args)
        poll_func_name = f"{func_symbol}_poll"
        prev_func = self.current_func_name
        has_return = self._function_contains_return(node)
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(node.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(node.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if node.args.vararg is not None:
            arg_nodes.append(node.args.vararg)
        arg_nodes.extend(kwonly)
        if node.args.kwarg is not None:
            arg_nodes.append(node.args.kwarg)

        free_vars: list[str] = []
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars(node)
            if free_vars:
                self.unbound_check_names.update(free_vars)
                for name in free_vars:
                    self._box_local(name)
                for name in free_vars:
                    hint = self.boxed_local_hints.get(name)
                    if hint is None:
                        value = self.locals.get(name)
                        if value is not None and value.type_hint:
                            hint = value.type_hint
                    free_var_hints[name] = hint or "Any"
                closure_items = [self.boxed_locals[name] for name in free_vars]
                closure_val = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(
                    MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                )
                has_closure = True

        # Add to globals to support calls from other scopes
        func_kind = "AsyncClosureFunc" if has_closure else "AsyncFunc"
        self.globals[func_name] = MoltValue(
            func_name, type_hint=f"{func_kind}:{poll_func_name}:0"
        )  # Placeholder size

        prev_state = self._capture_function_state()
        self.start_function(
            poll_func_name,
            params=["self"],
            type_facts_name=func_name,
            needs_return_slot=has_return,
        )
        self.async_context = True
        self.global_decls = self._collect_global_decls(node.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
        assigned = self._collect_assigned_names(node.body)
        self.del_targets = self._collect_deleted_names(node.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        if has_closure:
            self.async_closure_offset = 0
            self.async_locals_base = 8
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
        for i, arg in enumerate(arg_nodes):
            self.async_locals[arg.arg] = self.async_locals_base + i * 8
            if self._hints_enabled():
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
                if hint is not None:
                    self.async_local_hints[arg.arg] = hint
        self._store_return_slot_for_stateful()
        self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
        self._init_scope_async_locals(arg_nodes)
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
        self._push_qualname(func_name, True)
        try:
            for item in node.body:
                self.visit(item)
        finally:
            self._pop_qualname()
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
        self._spill_async_temporaries()
        closure_size = self.async_locals_base + len(self.async_locals) * 8
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        func_hint = f"{func_kind}:{poll_func_name}:{closure_size}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW", args=[func_symbol, len(params)], result=func_val
                )
            )
        func_spill = None
        if self.in_generator and self._signature_contains_yield(
            decorators=node.decorator_list,
            args=node.args,
            returns=node.returns,
        ):
            func_spill = self._spill_async_value(
                func_val, f"__func_meta_{len(self.async_locals)}"
            )
        self._emit_function_metadata(
            func_val,
            name=func_name,
            qualname=qualname,
            trace_lineno=node.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=node.args.defaults,
            kw_default_exprs=node.args.kw_defaults,
            docstring=ast.get_docstring(node),
            is_coroutine=True,
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        self._emit_function_annotate(func_val, node)
        closure_size_val = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[closure_size], result=closure_size_val))
        self.emit(
            MoltOp(
                kind="SETATTR_GENERIC_OBJ",
                args=[func_val, "__molt_closure_size__", closure_size_val],
                result=MoltValue("none"),
            )
        )
        if self.current_func_name == "molt_main":
            self.globals[func_name] = func_val
        else:
            self._store_local_value(func_name, func_val)
        self._emit_module_attr_set(func_name, func_val)

        prev_func = self.current_func_name
        prev_state = self._capture_function_state()
        func_params = params
        if has_closure:
            func_params = [_MOLT_CLOSURE_PARAM] + params
        self.start_function(
            func_symbol,
            params=func_params,
            type_facts_name=func_name,
        )
        if has_closure:
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        for idx, arg in enumerate(arg_nodes):
            hint = None
            if idx == 0 and arg.arg == "self":
                hint = None
            if self._hints_enabled():
                explicit = self.explicit_type_hints.get(arg.arg)
                if explicit is None:
                    explicit = self._annotation_to_hint(arg.annotation)
                    if explicit is not None:
                        self.explicit_type_hints[arg.arg] = explicit
                if explicit is not None:
                    hint = explicit
                elif hint is None:
                    hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "Unknown")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        args = [self.locals[arg.arg] for arg in arg_nodes]
        if has_closure:
            args = [self.locals[_MOLT_CLOSURE_PARAM]] + args
        res = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(
                kind="ALLOC_TASK",
                args=[poll_func_name, closure_size] + args,
                result=res,
                metadata={"task_kind": "future"},
            )
        )
        self.emit(MoltOp(kind="ret", args=[res], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        if node.decorator_list:
            decorated = func_val
            for deco in reversed(node.decorator_list):
                decorator_val = self.visit(deco)
                if decorator_val is None:
                    raise NotImplementedError("Unsupported decorator")
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC", args=[decorator_val, decorated], result=res
                    )
                )
                decorated = res
            func_val = decorated
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self._store_local_value(func_name, func_val)
            self._emit_module_attr_set(func_name, func_val)
        return None

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        if self.current_func_name == "molt_main":
            self.module_global_mutations.update(self._collect_global_decls(node.body))
        is_generator = self._function_contains_yield(node)
        has_return = self._function_contains_return(node)
        func_name = node.name
        qualname = self._qualname_for_def(func_name)
        if node.decorator_list and any(
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
            func_symbol = self._function_symbol(func_name)
            prev_func = self.current_func_name
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                node.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(node.args)
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
                qualname=qualname,
                trace_lineno=node.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=node.args.defaults,
                kw_default_exprs=node.args.kw_defaults,
                docstring=ast.get_docstring(node),
            )
            self._emit_function_annotate(func_val, node)
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self._store_local_value(func_name, func_val)
            self._emit_module_attr_set(func_name, func_val)
            prev_state = self._capture_function_state()
            self.start_function(func_symbol, params=params, type_facts_name=func_name)
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
        if is_generator:
            func_symbol = self._function_symbol(func_name)
            self._record_func_default_specs(func_symbol, node.args)
            poll_func_name = f"{func_symbol}_poll"
            prev_func = self.current_func_name
            posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(
                node.args
            )
            posonly_names = [arg.arg for arg in posonly]
            pos_or_kw_names = [arg.arg for arg in pos_or_kw]
            kwonly_names = [arg.arg for arg in kwonly]
            params = self._function_param_names(node.args)
            arg_nodes: list[ast.arg] = posonly + pos_or_kw
            if node.args.vararg is not None:
                arg_nodes.append(node.args.vararg)
            arg_nodes.extend(kwonly)
            if node.args.kwarg is not None:
                arg_nodes.append(node.args.kwarg)

            free_vars: list[str] = []
            free_var_hints: dict[str, str] = {}
            closure_val: MoltValue | None = None
            has_closure = False
            if self.current_func_name != "molt_main":
                free_vars = self._collect_free_vars(node)
                if free_vars:
                    self.unbound_check_names.update(free_vars)
                    for name in free_vars:
                        self._box_local(name)
                    for name in free_vars:
                        hint = self.boxed_local_hints.get(name)
                        if hint is None:
                            value = self.locals.get(name)
                            if value is not None and value.type_hint:
                                hint = value.type_hint
                        free_var_hints[name] = hint or "Any"
                    closure_items = [self.boxed_locals[name] for name in free_vars]
                    closure_val = MoltValue(self.next_var(), type_hint="tuple")
                    self.emit(
                        MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                    )
                    has_closure = True

            func_kind = "GenClosureFunc" if has_closure else "GenFunc"
            func_val = MoltValue(
                self.next_var(), type_hint=f"{func_kind}:{poll_func_name}:0"
            )
            if has_closure and closure_val is not None:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW_CLOSURE",
                        args=[poll_func_name, len(params), closure_val],
                        result=func_val,
                    )
                )
            else:
                self.emit(
                    MoltOp(
                        kind="FUNC_NEW",
                        args=[poll_func_name, len(params)],
                        result=func_val,
                    )
                )
            func_spill = None
            if self.in_generator and self._signature_contains_yield(
                decorators=node.decorator_list,
                args=node.args,
                returns=node.returns,
            ):
                func_spill = self._spill_async_value(
                    func_val, f"__func_meta_{len(self.async_locals)}"
                )
            self._emit_function_metadata(
                func_val,
                name=func_name,
                qualname=qualname,
                trace_lineno=node.lineno,
                posonly_params=posonly_names,
                pos_or_kw_params=pos_or_kw_names,
                kwonly_params=kwonly_names,
                vararg=vararg,
                varkw=varkw,
                default_exprs=node.args.defaults,
                kw_default_exprs=node.args.kw_defaults,
                docstring=ast.get_docstring(node),
                is_generator=True,
            )
            if func_spill is not None:
                func_val = self._reload_async_value(func_spill, func_val.type_hint)
            self._emit_function_annotate(func_val, node)
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self._store_local_value(func_name, func_val)
            self._emit_module_attr_set(func_name, func_val)

            prev_state = self._capture_function_state()
            self.start_function(
                poll_func_name,
                params=["self"],
                type_facts_name=func_name,
                needs_return_slot=has_return,
            )
            self.global_decls = self._collect_global_decls(node.body)
            self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
            assigned = self._collect_assigned_names(node.body)
            self.del_targets = self._collect_deleted_names(node.body)
            self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
            self.unbound_check_names = set(self.scope_assigned)
            self.in_generator = True
            if has_closure:
                self.async_closure_offset = GEN_CONTROL_SIZE
                self.async_locals_base = GEN_CONTROL_SIZE + 8
                self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
                self.free_var_hints = free_var_hints
            else:
                self.async_locals_base = GEN_CONTROL_SIZE
            for i, arg in enumerate(arg_nodes):
                self.async_locals[arg.arg] = self.async_locals_base + i * 8
                if self._hints_enabled():
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is None:
                        hint = self._annotation_to_hint(arg.annotation)
                        if hint is not None:
                            self.explicit_type_hints[arg.arg] = hint
            self._store_return_slot_for_stateful()
            self.emit(MoltOp(kind="STATE_SWITCH", args=[], result=MoltValue("none")))
            self._init_scope_async_locals(arg_nodes)
            if self.type_hint_policy == "check":
                for arg in arg_nodes:
                    hint = self.explicit_type_hints.get(arg.arg)
                    if hint is not None:
                        self._emit_guard_type(MoltValue(arg.arg, type_hint=hint), hint)
            self._push_qualname(func_name, True)
            try:
                for item in node.body:
                    self.visit(item)
                    if isinstance(item, (ast.Return, ast.Raise)):
                        break
            finally:
                self._pop_qualname()
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
            self._spill_async_temporaries()
            closure_size = self.async_locals_base + len(self.async_locals) * 8
            self.resume_function(prev_func)
            self._restore_function_state(prev_state)
            func_val.type_hint = f"{func_kind}:{poll_func_name}:{closure_size}"
            closure_size_val = MoltValue(self.next_var(), type_hint="int")
            self.emit(
                MoltOp(kind="CONST", args=[closure_size], result=closure_size_val)
            )
            self.emit(
                MoltOp(
                    kind="SETATTR_GENERIC_OBJ",
                    args=[func_val, "__molt_closure_size__", closure_size_val],
                    result=MoltValue("none"),
                )
            )
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self._store_local_value(func_name, func_val)
            if node.decorator_list:
                decorated = func_val
                for deco in reversed(node.decorator_list):
                    decorator_val = self.visit(deco)
                    if decorator_val is None:
                        raise NotImplementedError("Unsupported decorator")
                    res = MoltValue(self.next_var(), type_hint="Any")
                    self.emit(
                        MoltOp(
                            kind="CALL_FUNC",
                            args=[decorator_val, decorated],
                            result=res,
                        )
                    )
                    decorated = res
                func_val = decorated
                if self.current_func_name == "molt_main":
                    self.globals[func_name] = func_val
                else:
                    self._store_local_value(func_name, func_val)
                self._emit_module_attr_set(func_name, func_val)
            return None

        func_name = node.name
        func_symbol = self._function_symbol(func_name)
        self._record_func_default_specs(func_symbol, node.args)
        prev_func = self.current_func_name
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(node.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(node.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if node.args.vararg is not None:
            arg_nodes.append(node.args.vararg)
        arg_nodes.extend(kwonly)
        if node.args.kwarg is not None:
            arg_nodes.append(node.args.kwarg)

        free_vars: list[str] = []
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars(node)
            if free_vars:
                self.unbound_check_names.update(free_vars)
                for name in free_vars:
                    self._box_local(name)
                for name in free_vars:
                    hint = self.boxed_local_hints.get(name)
                    if hint is None:
                        value = self.locals.get(name)
                        if value is not None and value.type_hint:
                            hint = value.type_hint
                    free_var_hints[name] = hint or "Any"
                closure_items = [self.boxed_locals[name] for name in free_vars]
                closure_val = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(
                    MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                )
                has_closure = True

        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW", args=[func_symbol, len(params)], result=func_val
                )
            )
        func_spill = None
        if self.in_generator and self._signature_contains_yield(
            decorators=node.decorator_list,
            args=node.args,
            returns=node.returns,
        ):
            func_spill = self._spill_async_value(
                func_val, f"__func_meta_{len(self.async_locals)}"
            )
        self._emit_function_metadata(
            func_val,
            name=func_name,
            qualname=qualname,
            trace_lineno=node.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=node.args.defaults,
            kw_default_exprs=node.args.kw_defaults,
            docstring=ast.get_docstring(node),
        )
        if func_spill is not None:
            func_val = self._reload_async_value(func_spill, func_val.type_hint)
        self._emit_function_annotate(func_val, node)
        if self.current_func_name == "molt_main":
            self.globals[func_name] = func_val
        else:
            self._store_local_value(func_name, func_val)
        self._emit_module_attr_set(func_name, func_val)

        func_params = params
        if has_closure:
            func_params = [_MOLT_CLOSURE_PARAM] + params
        prev_state = self._capture_function_state()
        self.start_function(
            func_symbol,
            params=func_params,
            type_facts_name=func_name,
            needs_return_slot=False,
        )
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = self._collect_global_decls(node.body)
        self.nonlocal_decls = self._collect_nonlocal_decls(node.body)
        assigned = self._collect_assigned_names(node.body)
        self.del_targets = self._collect_deleted_names(node.body)
        self.scope_assigned = assigned - self.nonlocal_decls - self.global_decls
        self.unbound_check_names = set(self.scope_assigned)
        for arg in arg_nodes:
            hint = None
            if self.type_hint_policy == "ignore" and arg.annotation is not None:
                inferred = self._annotation_to_hint(arg.annotation)
                if inferred is not None and inferred in self.classes:
                    hint = inferred
            if self._hints_enabled():
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
            if hint is None and self._hints_enabled():
                hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "Unknown")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        if not self.is_async():
            for name in sorted(self.scope_assigned):
                self._box_local(name)
        self._push_qualname(func_name, True)
        try:
            for item in node.body:
                self.visit(item)
        finally:
            self._pop_qualname()
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
        if node.decorator_list:
            decorated = func_val
            for deco in reversed(node.decorator_list):
                decorator_val = self.visit(deco)
                if decorator_val is None:
                    raise NotImplementedError("Unsupported decorator")
                res = MoltValue(self.next_var(), type_hint="Any")
                self.emit(
                    MoltOp(
                        kind="CALL_FUNC", args=[decorator_val, decorated], result=res
                    )
                )
                decorated = res
            func_val = decorated
            if self.current_func_name == "molt_main":
                self.globals[func_name] = func_val
            else:
                self._store_local_value(func_name, func_val)
            self._emit_module_attr_set(func_name, func_val)
        return None

    def visit_Lambda(self, node: ast.Lambda) -> MoltValue:
        func_symbol = self._lambda_symbol()
        qualname = self._qualname_for_def("<lambda>")
        self._record_func_default_specs(func_symbol, node.args)
        prev_func = self.current_func_name
        posonly, pos_or_kw, kwonly, vararg, varkw = self._split_function_args(node.args)
        posonly_names = [arg.arg for arg in posonly]
        pos_or_kw_names = [arg.arg for arg in pos_or_kw]
        kwonly_names = [arg.arg for arg in kwonly]
        params = self._function_param_names(node.args)
        arg_nodes: list[ast.arg] = posonly + pos_or_kw
        if node.args.vararg is not None:
            arg_nodes.append(node.args.vararg)
        arg_nodes.extend(kwonly)
        if node.args.kwarg is not None:
            arg_nodes.append(node.args.kwarg)

        free_vars: list[str] = []
        free_var_hints: dict[str, str] = {}
        closure_val: MoltValue | None = None
        has_closure = False
        if self.current_func_name != "molt_main":
            free_vars = self._collect_free_vars_expr(node)
            if free_vars:
                self.unbound_check_names.update(free_vars)
                for name in free_vars:
                    self._box_local(name)
                for name in free_vars:
                    hint = self.boxed_local_hints.get(name)
                    if hint is None:
                        value = self.locals.get(name)
                        if value is not None and value.type_hint:
                            hint = value.type_hint
                    free_var_hints[name] = hint or "Any"
                closure_items = [self.boxed_locals[name] for name in free_vars]
                closure_val = MoltValue(self.next_var(), type_hint="tuple")
                self.emit(
                    MoltOp(kind="TUPLE_NEW", args=closure_items, result=closure_val)
                )
                has_closure = True

        func_hint = f"Func:{func_symbol}"
        if has_closure:
            func_hint = f"ClosureFunc:{func_symbol}"
        func_val = MoltValue(self.next_var(), type_hint=func_hint)
        if has_closure and closure_val is not None:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW_CLOSURE",
                    args=[func_symbol, len(params), closure_val],
                    result=func_val,
                )
            )
        else:
            self.emit(
                MoltOp(
                    kind="FUNC_NEW", args=[func_symbol, len(params)], result=func_val
                )
            )
        self._emit_function_metadata(
            func_val,
            name="<lambda>",
            qualname=qualname,
            trace_lineno=node.lineno,
            posonly_params=posonly_names,
            pos_or_kw_params=pos_or_kw_names,
            kwonly_params=kwonly_names,
            vararg=vararg,
            varkw=varkw,
            default_exprs=node.args.defaults,
            kw_default_exprs=node.args.kw_defaults,
            docstring=None,
        )

        func_params = params
        if has_closure:
            func_params = [_MOLT_CLOSURE_PARAM] + params
        prev_state = self._capture_function_state()
        self.start_function(
            func_symbol, params=func_params, type_facts_name=func_symbol
        )
        if has_closure:
            self.free_vars = {name: idx for idx, name in enumerate(free_vars)}
            self.free_var_hints = free_var_hints
            self.locals[_MOLT_CLOSURE_PARAM] = MoltValue(
                _MOLT_CLOSURE_PARAM, type_hint="tuple"
            )
        self.global_decls = set()
        for arg in arg_nodes:
            hint = None
            if self.type_hint_policy == "ignore" and arg.annotation is not None:
                inferred = self._annotation_to_hint(arg.annotation)
                if inferred is not None and inferred in self.classes:
                    hint = inferred
            if self._hints_enabled():
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is None:
                    hint = self._annotation_to_hint(arg.annotation)
                    if hint is not None:
                        self.explicit_type_hints[arg.arg] = hint
            if hint is None and self._hints_enabled():
                hint = "Any"
            value = MoltValue(arg.arg, type_hint=hint or "Unknown")
            if hint is not None:
                self._apply_hint_to_value(arg.arg, value, hint)
            self.locals[arg.arg] = value
        if self.type_hint_policy == "check":
            for arg in arg_nodes:
                hint = self.explicit_type_hints.get(arg.arg)
                if hint is not None:
                    self._emit_guard_type(self.locals[arg.arg], hint)
        if not self.is_async():
            for name in sorted(self.scope_assigned):
                self._box_local(name)
        self._push_qualname("<lambda>", True)
        try:
            val = self.visit(node.body)
        finally:
            self._pop_qualname()
        if val is None:
            val = MoltValue(self.next_var(), type_hint="None")
            self.emit(MoltOp(kind="CONST_NONE", args=[], result=val))
        self.emit(MoltOp(kind="ret", args=[val], result=MoltValue("none")))
        self.resume_function(prev_func)
        self._restore_function_state(prev_state)
        return func_val

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
            self.imported_modules[bind_name] = module_name
            if self.current_func_name == "molt_main":
                self.global_imported_modules[bind_name] = module_name
        return None

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.module is None:
            raise NotImplementedError("Relative imports are not supported yet")
        if node.module == "__future__":
            for alias in node.names:
                if alias.name == "annotations":
                    self.future_annotations = True
            return None
        if node.module == "typing_extensions":
            return None
        module_name = node.module
        module_val = self._emit_module_load_with_parents(module_name)
        for alias in node.names:
            if alias.name == "*":
                if self.current_func_name != "molt_main":
                    raise self.compat.unsupported(
                        node,
                        "import * only allowed at module level",
                        detail="from ... import *",
                    )
                if self.module_obj is None:
                    raise self.compat.unsupported(
                        node,
                        "import * requires module scope",
                        detail="module object missing",
                    )
                res = MoltValue(self.next_var(), type_hint="None")
                self.emit(
                    MoltOp(
                        kind="MODULE_IMPORT_STAR",
                        args=[module_val, self.module_obj],
                        result=res,
                    )
                )
                return None
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
            self.imported_names[bind_name] = module_name
            if self.current_func_name == "molt_main":
                self.global_imported_names[bind_name] = module_name
            self.locals[bind_name] = attr_val
            self.exact_locals.pop(bind_name, None)
            if self.current_func_name == "molt_main":
                self.globals[bind_name] = attr_val
            self._emit_module_attr_set(bind_name, attr_val)
            if self.known_modules:
                if submodule_name in self.known_modules:
                    self.imported_modules[bind_name] = submodule_name
                    if self.current_func_name == "molt_main":
                        self.global_imported_modules[bind_name] = submodule_name
                elif module_name == "molt.stdlib" and attr_name in self.known_modules:
                    stdlib_name = f"{module_name}.{attr_name}"
                    self.imported_modules[bind_name] = stdlib_name
                    if self.current_func_name == "molt_main":
                        self.global_imported_modules[bind_name] = stdlib_name
        return None

    def _emit_await_anext(
        self,
        iter_obj: MoltValue,
        *,
        default_val: MoltValue | None,
        has_default: bool,
    ) -> MoltValue:
        if iter_obj.type_hint in {"iter", "generator"}:
            pair = self._emit_iter_next_checked(iter_obj)
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
        self.emit(MoltOp(kind="STRING_EQ", args=[kind_val, stop_kind], result=is_stop))
        self.emit(MoltOp(kind="IF", args=[is_stop], result=MoltValue("none")))
        if not has_default:
            self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        else:
            self.emit(MoltOp(kind="EXCEPTION_CLEAR", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="RAISE", args=[exc_val], result=MoltValue("none")))
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        self.emit(MoltOp(kind="ELSE", args=[], result=MoltValue("none")))
        awaited_val = self._emit_await_value(awaitable, raise_pending=False)
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
            MoltOp(
                kind="STRING_EQ", args=[kind_after, stop_after], result=is_stop_after
            )
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
            awaitable_new = self._emit_awaitable_transform(awaitable_new)
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
            coro = self._emit_awaitable_transform(coro)
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
        self._emit_raise_if_pending()
        return res

    def _emit_awaitable_transform(self, awaitable: MoltValue) -> MoltValue:
        name_val = MoltValue(self.next_var(), type_hint="str")
        self.emit(MoltOp(kind="CONST_STR", args=["__await__"], result=name_val))
        cell = MoltValue(self.next_var(), type_hint="list")
        self.emit(MoltOp(kind="LIST_NEW", args=[awaitable], result=cell))
        zero = MoltValue(self.next_var(), type_hint="int")
        self.emit(MoltOp(kind="CONST", args=[0], result=zero))
        has_attr = MoltValue(self.next_var(), type_hint="bool")
        self.emit(
            MoltOp(kind="HASATTR_NAME", args=[awaitable, name_val], result=has_attr)
        )
        self.emit(MoltOp(kind="IF", args=[has_attr], result=MoltValue("none")))
        method = MoltValue(self.next_var(), type_hint="Any")
        self.emit(
            MoltOp(kind="GETATTR_NAME", args=[awaitable, name_val], result=method)
        )
        awaited = self._emit_call_bound_or_func(method, [])
        self.emit(
            MoltOp(
                kind="STORE_INDEX",
                args=[cell, zero, awaited],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
        final_val = MoltValue(self.next_var(), type_hint="Any")
        self.emit(MoltOp(kind="INDEX", args=[cell, zero], result=final_val))
        return final_val

    def _emit_await_value(
        self, awaitable: MoltValue, *, raise_pending: bool = True
    ) -> MoltValue:
        if not self.is_async():
            raise NotImplementedError("await outside async function")
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
        self.emit(MoltOp(kind="IF", args=[is_empty_cached], result=MoltValue("none")))
        transformed = self._emit_awaitable_transform(awaitable)
        self.emit(
            MoltOp(
                kind="STORE_CLOSURE",
                args=["self", awaitable_slot, transformed],
                result=MoltValue("none"),
            )
        )
        self.emit(MoltOp(kind="END_IF", args=[], result=MoltValue("none")))
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
        coro = MoltValue(self.next_var(), type_hint="Future")
        self.emit(
            MoltOp(
                kind="LOAD_CLOSURE",
                args=["self", awaitable_slot],
                result=coro,
            )
        )
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
        self.emit(MoltOp(kind="LOAD_CLOSURE", args=["self", result_slot], result=res))
        if raise_pending:
            self._emit_raise_if_pending()
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
        pair = self._emit_iter_next_checked(iter_obj)
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
        pair = self._emit_iter_next_checked(iter_obj)
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

        def field_offset(expected_class: str, attr: str) -> int | None:
            class_info = self.classes.get(expected_class)
            if not class_info:
                return None
            return class_info.get("fields", {}).get(attr)

        for op in ops:
            if op.kind == "CONST":
                value = op.args[0]
                if isinstance(value, bool):
                    value = 1 if value else 0
                json_ops.append(
                    {"kind": "const", "value": value, "out": op.result.name}
                )
            elif op.kind == "CONST_BIGINT":
                json_ops.append(
                    {
                        "kind": "const_bigint",
                        "s_value": op.args[0],
                        "out": op.result.name,
                    }
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
            elif op.kind == "CONST_NOT_IMPLEMENTED":
                json_ops.append(
                    {"kind": "const_not_implemented", "out": op.result.name}
                )
            elif op.kind == "CONST_ELLIPSIS":
                json_ops.append({"kind": "const_ellipsis", "out": op.result.name})
            elif op.kind == "ADD":
                add_entry: dict[str, Any] = {
                    "kind": "add",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    add_entry["fast_int"] = True
                json_ops.append(add_entry)
            elif op.kind == "INPLACE_ADD":
                add_entry = {
                    "kind": "inplace_add",
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
            elif op.kind == "INPLACE_SUB":
                sub_entry = {
                    "kind": "inplace_sub",
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
            elif op.kind == "INPLACE_MUL":
                mul_entry = {
                    "kind": "inplace_mul",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    mul_entry["fast_int"] = True
                json_ops.append(mul_entry)
            elif op.kind == "DIV":
                json_ops.append(
                    {
                        "kind": "div",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FLOORDIV":
                json_ops.append(
                    {
                        "kind": "floordiv",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MOD":
                json_ops.append(
                    {
                        "kind": "mod",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "POW":
                json_ops.append(
                    {
                        "kind": "pow",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BIT_OR":
                json_ops.append(
                    {
                        "kind": "bit_or",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INPLACE_BIT_OR":
                json_ops.append(
                    {
                        "kind": "inplace_bit_or",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BIT_AND":
                json_ops.append(
                    {
                        "kind": "bit_and",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INPLACE_BIT_AND":
                json_ops.append(
                    {
                        "kind": "inplace_bit_and",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BIT_XOR":
                json_ops.append(
                    {
                        "kind": "bit_xor",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INPLACE_BIT_XOR":
                json_ops.append(
                    {
                        "kind": "inplace_bit_xor",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LSHIFT":
                json_ops.append(
                    {
                        "kind": "lshift",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "RSHIFT":
                json_ops.append(
                    {
                        "kind": "rshift",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MATMUL":
                json_ops.append(
                    {
                        "kind": "matmul",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "POW_MOD":
                json_ops.append(
                    {
                        "kind": "pow_mod",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ROUND":
                json_ops.append(
                    {
                        "kind": "round",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TRUNC":
                json_ops.append(
                    {
                        "kind": "trunc",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LT":
                lt_entry: dict[str, Any] = {
                    "kind": "lt",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    lt_entry["fast_int"] = True
                json_ops.append(lt_entry)
            elif op.kind == "LE":
                json_ops.append(
                    {
                        "kind": "le",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GT":
                json_ops.append(
                    {
                        "kind": "gt",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GE":
                json_ops.append(
                    {
                        "kind": "ge",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "EQ":
                eq_entry: dict[str, Any] = {
                    "kind": "eq",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    eq_entry["fast_int"] = True
                json_ops.append(eq_entry)
            elif op.kind == "NE":
                ne_entry: dict[str, Any] = {
                    "kind": "ne",
                    "args": [arg.name for arg in op.args],
                    "out": op.result.name,
                }
                if self._should_fast_int(op):
                    ne_entry["fast_int"] = True
                json_ops.append(ne_entry)
            elif op.kind == "STRING_EQ":
                json_ops.append(
                    {
                        "kind": "string_eq",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IS":
                json_ops.append(
                    {
                        "kind": "is",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INVERT":
                json_ops.append(
                    {
                        "kind": "invert",
                        "args": [op.args[0].name],
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
            elif op.kind == "LINE":
                json_ops.append({"kind": "line", "value": int(op.args[0])})
            elif op.kind == "CALL":
                target = op.args[0]
                code_id = self.func_code_ids.get(target, 0)
                json_ops.append(
                    {
                        "kind": "call",
                        "s_value": target,
                        "args": [arg.name for arg in op.args[1:]],
                        "value": code_id,
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
            elif op.kind == "CALL_BIND":
                json_ops.append(
                    {
                        "kind": "call_bind",
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
            elif op.kind == "BUILTIN_FUNC":
                func_name, arity = op.args
                json_ops.append(
                    {
                        "kind": "builtin_func",
                        "s_value": func_name,
                        "value": arity,
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
            elif op.kind == "FUNC_NEW_CLOSURE":
                func_name, arity, closure = op.args
                json_ops.append(
                    {
                        "kind": "func_new_closure",
                        "s_value": func_name,
                        "value": arity,
                        "args": [closure.name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CODE_NEW":
                json_ops.append(
                    {
                        "kind": "code_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CODE_SLOT_SET":
                code_id = 0
                if op.metadata and "code_id" in op.metadata:
                    code_id = int(op.metadata["code_id"])
                json_ops.append(
                    {
                        "kind": "code_slot_set",
                        "value": code_id,
                        "args": [op.args[0].name],
                    }
                )
            elif op.kind == "CODE_SLOTS_INIT":
                json_ops.append(
                    {
                        "kind": "code_slots_init",
                        "value": int(op.args[0]),
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
            elif op.kind == "MISSING":
                json_ops.append(
                    {
                        "kind": "missing",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUNCTION_CLOSURE_BITS":
                json_ops.append(
                    {
                        "kind": "function_closure_bits",
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
            elif op.kind == "CLASS_VERSION":
                json_ops.append(
                    {
                        "kind": "class_layout_version",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CLASS_SET_LAYOUT_VERSION":
                json_ops.append(
                    {
                        "kind": "class_set_layout_version",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "GUARD_LAYOUT":
                json_ops.append(
                    {
                        "kind": "guard_layout",
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
            elif op.kind == "MODULE_GET_GLOBAL":
                json_ops.append(
                    {
                        "kind": "module_get_global",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "MODULE_DEL_GLOBAL":
                json_ops.append(
                    {
                        "kind": "module_del_global",
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
            elif op.kind == "MODULE_IMPORT_STAR":
                json_ops.append(
                    {
                        "kind": "module_import_star",
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
            elif op.kind == "EXCEPTION_STACK_CLEAR":
                json_ops.append(
                    {"kind": "exception_stack_clear", "out": op.result.name}
                )
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
            elif op.kind == "EXCEPTION_NEW_FROM_CLASS":
                json_ops.append(
                    {
                        "kind": "exception_new_from_class",
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
            elif op.kind == "EXCEPTION_SET_LAST":
                json_ops.append(
                    {
                        "kind": "exception_set_last",
                        "args": [op.args[0].name],
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
            elif op.kind == "EXCEPTION_CLASS":
                json_ops.append(
                    {
                        "kind": "exception_class",
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
            elif op.kind == "ALLOC_CLASS":
                class_ref, class_id = op.args
                json_ops.append(
                    {
                        "kind": "alloc_class",
                        "args": [class_ref.name],
                        "value": self.classes[class_id]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ALLOC_CLASS_TRUSTED":
                class_ref, class_id = op.args
                json_ops.append(
                    {
                        "kind": "alloc_class_trusted",
                        "args": [class_ref.name],
                        "value": self.classes[class_id]["size"],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ALLOC_CLASS_STATIC":
                class_ref, class_id = op.args
                json_ops.append(
                    {
                        "kind": "alloc_class_static",
                        "args": [class_ref.name],
                        "value": self.classes[class_id]["size"],
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
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "store",
                            "args": [obj.name, val.name],
                            "value": offset,
                        }
                    )
            elif op.kind == "SETATTR_INIT":
                obj, attr, val, *rest = op.args
                if rest:
                    expected_class = rest[0]
                else:
                    expected_class = list(self.classes.keys())[-1]
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "store_init",
                            "args": [obj.name, val.name],
                            "value": offset,
                        }
                    )
            elif op.kind == "GUARDED_SETATTR":
                obj, class_ref, expected_version, attr, val, expected_class = op.args
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "guarded_field_set",
                            "args": [
                                obj.name,
                                class_ref.name,
                                expected_version.name,
                                val.name,
                            ],
                            "s_value": attr,
                            "value": offset,
                            "out": op.result.name,
                        }
                    )
            elif op.kind == "GUARDED_SETATTR_INIT":
                obj, class_ref, expected_version, attr, val, expected_class = op.args
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_obj",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "set_attr_generic_ptr",
                                "args": [obj.name, val.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "guarded_field_init",
                            "args": [
                                obj.name,
                                class_ref.name,
                                expected_version.name,
                                val.name,
                            ],
                            "s_value": attr,
                            "value": offset,
                            "out": op.result.name,
                        }
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
                obj, attr, *rest = op.args
                if rest:
                    expected_class = rest[0]
                else:
                    expected_class = list(self.classes.keys())[-1]
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_obj",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_ptr",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "load",
                            "args": [obj.name],
                            "value": offset,
                            "out": op.result.name,
                        }
                    )
            elif op.kind == "GUARDED_GETATTR":
                obj, class_ref, expected_version, attr, expected_class = op.args
                offset = field_offset(expected_class, attr)
                if offset is None:
                    class_info = self.classes.get(expected_class)
                    if class_info and self._class_is_exception_subclass(
                        expected_class, class_info
                    ):
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_obj",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                    else:
                        json_ops.append(
                            {
                                "kind": "get_attr_generic_ptr",
                                "args": [obj.name],
                                "s_value": attr,
                                "out": op.result.name,
                            }
                        )
                else:
                    json_ops.append(
                        {
                            "kind": "guarded_field_get",
                            "args": [obj.name, class_ref.name, expected_version.name],
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
            elif op.kind == "GETATTR_SPECIAL_OBJ":
                json_ops.append(
                    {
                        "kind": "get_attr_special_obj",
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
            elif op.kind == "ID":
                json_ops.append(
                    {
                        "kind": "id",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ORD":
                json_ops.append(
                    {
                        "kind": "ord",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CHR":
                json_ops.append(
                    {
                        "kind": "chr",
                        "args": [
                            arg.name if hasattr(arg, "name") else str(arg)
                            for arg in op.args
                        ],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_NEW":
                json_ops.append(
                    {
                        "kind": "callargs_new",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_PUSH_POS":
                json_ops.append(
                    {
                        "kind": "callargs_push_pos",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_PUSH_KW":
                json_ops.append(
                    {
                        "kind": "callargs_push_kw",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_EXPAND_STAR":
                json_ops.append(
                    {
                        "kind": "callargs_expand_star",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CALLARGS_EXPAND_KWSTAR":
                json_ops.append(
                    {
                        "kind": "callargs_expand_kwstar",
                        "args": [arg.name for arg in op.args],
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
            elif op.kind == "LIST_CLEAR":
                json_ops.append(
                    {
                        "kind": "list_clear",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_COPY":
                json_ops.append(
                    {
                        "kind": "list_copy",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "LIST_REVERSE":
                json_ops.append(
                    {
                        "kind": "list_reverse",
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
            elif op.kind == "LIST_INDEX_RANGE":
                json_ops.append(
                    {
                        "kind": "list_index_range",
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
            elif op.kind == "BYTES_FROM_STR":
                json_ops.append(
                    {
                        "kind": "bytes_from_str",
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
            elif op.kind == "BYTEARRAY_FROM_STR":
                json_ops.append(
                    {
                        "kind": "bytearray_from_str",
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
            elif op.kind == "FLOAT_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "float_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "INT_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "int_from_obj",
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
            elif op.kind == "DICT_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "dict_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_NEW":
                json_ops.append(
                    {
                        "kind": "set_new",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FROZENSET_NEW":
                json_ops.append(
                    {
                        "kind": "frozenset_new",
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
            elif op.kind == "DICT_SETDEFAULT":
                json_ops.append(
                    {
                        "kind": "dict_setdefault",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_UPDATE":
                json_ops.append(
                    {
                        "kind": "dict_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_UPDATE_KWSTAR":
                json_ops.append(
                    {
                        "kind": "dict_update_kwstar",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_CLEAR":
                json_ops.append(
                    {
                        "kind": "dict_clear",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_COPY":
                json_ops.append(
                    {
                        "kind": "dict_copy",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "DICT_POPITEM":
                json_ops.append(
                    {
                        "kind": "dict_popitem",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_ADD":
                json_ops.append(
                    {
                        "kind": "set_add",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FROZENSET_ADD":
                json_ops.append(
                    {
                        "kind": "frozenset_add",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_DISCARD":
                json_ops.append(
                    {
                        "kind": "set_discard",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_REMOVE":
                json_ops.append(
                    {
                        "kind": "set_remove",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_POP":
                json_ops.append(
                    {
                        "kind": "set_pop",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_INTERSECTION_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_intersection_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_DIFFERENCE_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_difference_update",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "SET_SYMDIFF_UPDATE":
                json_ops.append(
                    {
                        "kind": "set_symdiff_update",
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
            elif op.kind == "ENUMERATE":
                json_ops.append(
                    {
                        "kind": "enumerate",
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
            elif op.kind == "DEL_INDEX":
                json_ops.append(
                    {
                        "kind": "del_index",
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
            elif op.kind == "LOOP_BREAK":
                json_ops.append({"kind": "loop_break"})
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
            elif op.kind == "BYTES_FIND_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_find_slice",
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
            elif op.kind == "BYTEARRAY_FIND_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_find_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_STARTSWITH":
                json_ops.append(
                    {
                        "kind": "bytes_startswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_STARTSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_startswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_STARTSWITH":
                json_ops.append(
                    {
                        "kind": "bytearray_startswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_STARTSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_startswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_ENDSWITH":
                json_ops.append(
                    {
                        "kind": "bytes_endswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_ENDSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_endswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_ENDSWITH":
                json_ops.append(
                    {
                        "kind": "bytearray_endswith",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_ENDSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_endswith_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_COUNT":
                json_ops.append(
                    {
                        "kind": "bytes_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_COUNT":
                json_ops.append(
                    {
                        "kind": "bytearray_count",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTES_COUNT_SLICE":
                json_ops.append(
                    {
                        "kind": "bytes_count_slice",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "BYTEARRAY_COUNT_SLICE":
                json_ops.append(
                    {
                        "kind": "bytearray_count_slice",
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
            elif op.kind == "REPR_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "repr_from_obj",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASCII_FROM_OBJ":
                json_ops.append(
                    {
                        "kind": "ascii_from_obj",
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
            elif op.kind == "STRING_FIND_SLICE":
                json_ops.append(
                    {
                        "kind": "string_find_slice",
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
            elif op.kind == "STRING_STARTSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "string_startswith_slice",
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
            elif op.kind == "STRING_ENDSWITH_SLICE":
                json_ops.append(
                    {
                        "kind": "string_endswith_slice",
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
            elif op.kind == "STRING_COUNT_SLICE":
                json_ops.append(
                    {
                        "kind": "string_count_slice",
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
            elif op.kind == "STRING_SPLIT_MAX":
                json_ops.append(
                    {
                        "kind": "string_split_max",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_LOWER":
                json_ops.append(
                    {
                        "kind": "string_lower",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_UPPER":
                json_ops.append(
                    {
                        "kind": "string_upper",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_CAPITALIZE":
                json_ops.append(
                    {
                        "kind": "string_capitalize",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_STRIP":
                json_ops.append(
                    {
                        "kind": "string_strip",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_LSTRIP":
                json_ops.append(
                    {
                        "kind": "string_lstrip",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "STRING_RSTRIP":
                json_ops.append(
                    {
                        "kind": "string_rstrip",
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
            elif op.kind == "BYTES_SPLIT_MAX":
                json_ops.append(
                    {
                        "kind": "bytes_split_max",
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
            elif op.kind == "BYTEARRAY_SPLIT_MAX":
                json_ops.append(
                    {
                        "kind": "bytearray_split_max",
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
            elif op.kind == "ALLOC_TASK":
                poll_func = op.args[0]
                size = op.args[1]
                args = op.args[2:]
                task_kind = op.metadata.get("task_kind") if op.metadata else None
                if task_kind not in {"future", "generator"}:
                    raise ValueError(
                        f"ALLOC_TASK missing task_kind metadata: {task_kind!r}"
                    )
                json_ops.append(
                    {
                        "kind": "alloc_task",
                        "s_value": poll_func,
                        "value": size,
                        "task_kind": task_kind,
                        "args": [arg.name for arg in args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASYNCGEN_NEW":
                json_ops.append(
                    {
                        "kind": "asyncgen_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "ASYNCGEN_SHUTDOWN":
                json_ops.append(
                    {
                        "kind": "asyncgen_shutdown",
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
            elif op.kind == "CANCEL_TOKEN_NEW":
                json_ops.append(
                    {
                        "kind": "cancel_token_new",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_CLONE":
                json_ops.append(
                    {
                        "kind": "cancel_token_clone",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_DROP":
                json_ops.append(
                    {
                        "kind": "cancel_token_drop",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_CANCEL":
                json_ops.append(
                    {
                        "kind": "cancel_token_cancel",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUTURE_CANCEL":
                json_ops.append(
                    {
                        "kind": "future_cancel",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUTURE_CANCEL_MSG":
                json_ops.append(
                    {
                        "kind": "future_cancel_msg",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "FUTURE_CANCEL_CLEAR":
                json_ops.append(
                    {
                        "kind": "future_cancel_clear",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROMISE_NEW":
                json_ops.append(
                    {
                        "kind": "promise_new",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROMISE_SET_RESULT":
                json_ops.append(
                    {
                        "kind": "promise_set_result",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "PROMISE_SET_EXCEPTION":
                json_ops.append(
                    {
                        "kind": "promise_set_exception",
                        "args": [op.args[0].name, op.args[1].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "THREAD_SUBMIT":
                json_ops.append(
                    {
                        "kind": "thread_submit",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "TASK_REGISTER_TOKEN_OWNED":
                json_ops.append(
                    {
                        "kind": "task_register_token_owned",
                        "args": [arg.name for arg in op.args],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_IS_CANCELLED":
                json_ops.append(
                    {
                        "kind": "cancel_token_is_cancelled",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_SET_CURRENT":
                json_ops.append(
                    {
                        "kind": "cancel_token_set_current",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_TOKEN_GET_CURRENT":
                json_ops.append(
                    {
                        "kind": "cancel_token_get_current",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCELLED":
                json_ops.append(
                    {
                        "kind": "cancelled",
                        "out": op.result.name,
                    }
                )
            elif op.kind == "CANCEL_CURRENT":
                json_ops.append(
                    {
                        "kind": "cancel_current",
                        "out": op.result.name,
                    }
                )
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
            elif op.kind == "CHAN_DROP":
                json_ops.append(
                    {
                        "kind": "chan_drop",
                        "args": [op.args[0].name],
                    }
                )
            elif op.kind == "CALL_ASYNC":
                poll_name = op.args[0]
                payload_args = op.args[1:] if len(op.args) > 1 else []
                json_ops.append(
                    {
                        "kind": "call_async",
                        "s_value": poll_name,
                        "args": [arg.name for arg in payload_args],
                        "out": op.result.name,
                    }
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
            elif op.kind == "IS_BOUND_METHOD":
                json_ops.append(
                    {
                        "kind": "is_bound_method",
                        "args": [op.args[0].name],
                        "out": op.result.name,
                    }
                )
            elif op.kind == "IS_CALLABLE":
                json_ops.append(
                    {
                        "kind": "is_callable",
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

    def _finalize_code_ids(self) -> None:
        for data in self.funcs_map.values():
            for op in data["ops"]:
                if op.kind == "CALL" and op.args:
                    target = op.args[0]
                    if isinstance(target, str):
                        self._register_code_symbol(target)

    def _ensure_code_slots_init(self) -> None:
        if self.code_slots_emitted:
            return
        self.code_slots_emitted = True
        count = len(self.func_code_ids)
        init_op = MoltOp(
            kind="CODE_SLOTS_INIT",
            args=[count],
            result=MoltValue("none"),
        )
        ops = self.funcs_map.get("molt_main", {}).get("ops")
        if ops is not None:
            ops.insert(0, init_op)

    def to_json(self) -> dict[str, Any]:
        self._finalize_code_ids()
        self._ensure_code_slots_init()
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
