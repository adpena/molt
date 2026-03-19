"""CPython 3.12 multiprocessing API-shape helpers for Molt."""

from __future__ import annotations

import builtins as _builtins
from abc import ABCMeta
import itertools as _itertools
import struct as _struct
import types as _types
import weakref as _weakref

from _intrinsics import require_intrinsic as _require_intrinsic


_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")


class _MethodCarrier:
    __slots__ = ("_fn",)

    def __init__(self, fn):
        self._fn = fn

    def call(self, *args, **kwargs):
        return self._fn(*args, **kwargs)


def _placeholder_function(module_name: str, symbol_name: str):
    def _fn(*_args, **_kwargs):
        raise RuntimeError(
            f'stdlib module "{module_name}" symbol "{symbol_name}" is not fully lowered yet'
        )

    _fn.__name__ = symbol_name
    return _fn


def _placeholder_type(symbol_name: str):
    base = Exception if symbol_name.endswith(("Error", "Exception")) else object
    return type(symbol_name, (base,), {})


def _placeholder_method(module_name: str, symbol_name: str):
    return _MethodCarrier(_placeholder_function(module_name, symbol_name)).call


def _make_value(
    module_name: str, symbol_name: str, type_name: str, expected_callable: bool
):
    if type_name == "module":
        return _types
    if type_name == "int":
        return 0
    if type_name == "float":
        return 0.0
    if type_name == "str":
        return ""
    if type_name == "bool":
        return False
    if type_name == "list":
        return []
    if type_name == "dict":
        return {}
    if type_name == "type":
        return _placeholder_type(symbol_name)
    if type_name == "function":
        return _placeholder_function(module_name, symbol_name)
    if type_name == "method":
        return _placeholder_method(module_name, symbol_name)
    if type_name == "builtin_function_or_method":
        return _builtins.len
    if type_name == "Struct":
        return _struct.Struct("<i")
    if type_name == "count":
        return _itertools.count()
    if type_name == "ABCMeta":
        return ABCMeta(symbol_name, (), {})
    if type_name == "WeakKeyDictionary":
        return _weakref.WeakKeyDictionary()
    value = (
        _placeholder_function(module_name, symbol_name)
        if expected_callable
        else object()
    )
    return value


def _type_and_callable(value) -> tuple[str, bool]:
    return type(value).__name__, bool(callable(value))


def apply_module_api_surface(
    module_name: str,
    namespace: dict[str, object],
    providers: dict[str, object] | None = None,
    prune: bool = False,
) -> None:
    rows = ROWS_BY_MODULE.get(module_name)
    if rows is None:
        return
    provider_map = providers or {}
    expected_names: set[str] = set()
    for symbol_name, type_name, expected_callable in rows:
        expected_names.add(symbol_name)
        if symbol_name in provider_map:
            value = provider_map[symbol_name]
        else:
            existing = namespace.get(symbol_name)
            if existing is not None:
                existing_type, existing_callable = _type_and_callable(existing)
                if (
                    existing_type == type_name
                    and existing_callable == expected_callable
                ):
                    continue
            value = _make_value(module_name, symbol_name, type_name, expected_callable)
        namespace[symbol_name] = value

    if prune:
        for key in list(namespace):
            if key.startswith("_"):
                continue
            if key not in expected_names:
                namespace.pop(key, None)


ROWS_BY_MODULE: dict[str, list[tuple[str, str, bool]]] = {
    "multiprocessing": [
        ("Array", "method", True),
        ("AuthenticationError", "type", True),
        ("Barrier", "method", True),
        ("BoundedSemaphore", "method", True),
        ("BufferTooShort", "type", True),
        ("Condition", "method", True),
        ("Event", "method", True),
        ("JoinableQueue", "method", True),
        ("Lock", "method", True),
        ("Manager", "method", True),
        ("Pipe", "method", True),
        ("Pool", "method", True),
        ("Process", "type", True),
        ("ProcessError", "type", True),
        ("Queue", "method", True),
        ("RLock", "method", True),
        ("RawArray", "method", True),
        ("RawValue", "method", True),
        ("SUBDEBUG", "int", False),
        ("SUBWARNING", "int", False),
        ("Semaphore", "method", True),
        ("SimpleQueue", "method", True),
        ("TimeoutError", "type", True),
        ("Value", "method", True),
        ("active_children", "function", True),
        ("allow_connection_pickling", "method", True),
        ("context", "module", False),
        ("cpu_count", "method", True),
        ("current_process", "function", True),
        ("freeze_support", "method", True),
        ("get_all_start_methods", "method", True),
        ("get_context", "method", True),
        ("get_logger", "method", True),
        ("get_start_method", "method", True),
        ("log_to_stderr", "method", True),
        ("parent_process", "function", True),
        ("process", "module", False),
        ("reducer", "module", False),
        ("reduction", "module", False),
        ("set_executable", "method", True),
        ("set_forkserver_preload", "method", True),
        ("set_start_method", "method", True),
        ("sys", "module", False),
    ],
    "multiprocessing.connection": [
        ("AuthenticationError", "type", True),
        ("BUFSIZE", "int", False),
        ("BufferTooShort", "type", True),
        ("CONNECTION_TIMEOUT", "float", False),
        ("Client", "function", True),
        ("Connection", "type", True),
        ("ConnectionWrapper", "type", True),
        ("Listener", "type", True),
        ("MESSAGE_LENGTH", "int", False),
        ("Pipe", "function", True),
        ("SocketClient", "function", True),
        ("SocketListener", "type", True),
        ("XmlClient", "function", True),
        ("XmlListener", "type", True),
        ("address_type", "function", True),
        ("answer_challenge", "function", True),
        ("arbitrary_address", "function", True),
        ("default_family", "str", False),
        ("deliver_challenge", "function", True),
        ("errno", "module", False),
        ("families", "list", False),
        ("io", "module", False),
        ("itertools", "module", False),
        ("os", "module", False),
        ("rebuild_connection", "function", True),
        ("reduce_connection", "function", True),
        ("reduction", "module", False),
        ("selectors", "module", False),
        ("socket", "module", False),
        ("struct", "module", False),
        ("sys", "module", False),
        ("tempfile", "module", False),
        ("time", "module", False),
        ("util", "module", False),
        ("wait", "function", True),
    ],
    "multiprocessing.context": [
        ("AuthenticationError", "type", True),
        ("BaseContext", "type", True),
        ("BufferTooShort", "type", True),
        ("DefaultContext", "type", True),
        ("ForkContext", "type", True),
        ("ForkProcess", "type", True),
        ("ForkServerContext", "type", True),
        ("ForkServerProcess", "type", True),
        ("Process", "type", True),
        ("ProcessError", "type", True),
        ("SpawnContext", "type", True),
        ("SpawnProcess", "type", True),
        ("TimeoutError", "type", True),
        ("assert_spawning", "function", True),
        ("get_spawning_popen", "function", True),
        ("os", "module", False),
        ("process", "module", False),
        ("reduction", "module", False),
        ("set_spawning_popen", "function", True),
        ("sys", "module", False),
        ("threading", "module", False),
    ],
    "multiprocessing.dummy": [
        ("Array", "function", True),
        ("Barrier", "type", True),
        ("BoundedSemaphore", "type", True),
        ("Condition", "type", True),
        ("DummyProcess", "type", True),
        ("Event", "type", True),
        ("JoinableQueue", "type", True),
        ("Lock", "builtin_function_or_method", True),
        ("Manager", "function", True),
        ("Namespace", "type", True),
        ("Pipe", "function", True),
        ("Pool", "function", True),
        ("Process", "type", True),
        ("Queue", "type", True),
        ("RLock", "function", True),
        ("Semaphore", "type", True),
        ("Value", "type", True),
        ("active_children", "function", True),
        ("array", "module", False),
        ("connection", "module", False),
        ("current_process", "function", True),
        ("dict", "type", True),
        ("freeze_support", "function", True),
        ("list", "type", True),
        ("shutdown", "function", True),
        ("sys", "module", False),
        ("threading", "module", False),
        ("weakref", "module", False),
    ],
    "multiprocessing.dummy.connection": [
        ("Client", "function", True),
        ("Connection", "type", True),
        ("Listener", "type", True),
        ("Pipe", "function", True),
        ("Queue", "type", True),
        ("families", "list", False),
    ],
    "multiprocessing.forkserver": [
        ("ForkServer", "type", True),
        ("MAXFDS_TO_SEND", "int", False),
        ("SIGNED_STRUCT", "Struct", False),
        ("connect_to_new_process", "method", True),
        ("connection", "module", False),
        ("ensure_running", "method", True),
        ("errno", "module", False),
        ("get_inherited_fds", "method", True),
        ("main", "function", True),
        ("os", "module", False),
        ("process", "module", False),
        ("read_signed", "function", True),
        ("reduction", "module", False),
        ("resource_tracker", "module", False),
        ("selectors", "module", False),
        ("set_forkserver_preload", "method", True),
        ("signal", "module", False),
        ("socket", "module", False),
        ("spawn", "module", False),
        ("struct", "module", False),
        ("sys", "module", False),
        ("threading", "module", False),
        ("util", "module", False),
        ("warnings", "module", False),
        ("write_signed", "function", True),
    ],
    "multiprocessing.heap": [
        ("Arena", "type", True),
        ("BufferWrapper", "type", True),
        ("Heap", "type", True),
        ("assert_spawning", "function", True),
        ("bisect", "module", False),
        ("defaultdict", "type", True),
        ("mmap", "module", False),
        ("os", "module", False),
        ("rebuild_arena", "function", True),
        ("reduce_arena", "function", True),
        ("reduction", "module", False),
        ("sys", "module", False),
        ("tempfile", "module", False),
        ("threading", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.managers": [
        ("AcquirerProxy", "type", True),
        ("Array", "function", True),
        ("ArrayProxy", "type", True),
        ("AutoProxy", "function", True),
        ("BarrierProxy", "type", True),
        ("BaseListProxy", "type", True),
        ("BaseManager", "type", True),
        ("BasePoolProxy", "type", True),
        ("BaseProxy", "type", True),
        ("ConditionProxy", "type", True),
        ("DictProxy", "type", True),
        ("EventProxy", "type", True),
        ("HAS_SHMEM", "bool", False),
        ("IteratorProxy", "type", True),
        ("ListProxy", "type", True),
        ("MakeProxyType", "function", True),
        ("Namespace", "type", True),
        ("NamespaceProxy", "type", True),
        ("PoolProxy", "type", True),
        ("ProcessError", "type", True),
        ("ProcessLocalSet", "type", True),
        ("RebuildProxy", "function", True),
        ("RemoteError", "type", True),
        ("Server", "type", True),
        ("SharedMemoryManager", "type", True),
        ("SharedMemoryServer", "type", True),
        ("State", "type", True),
        ("SyncManager", "type", True),
        ("Token", "type", True),
        ("Value", "type", True),
        ("ValueProxy", "type", True),
        ("all_methods", "function", True),
        ("array", "module", False),
        ("connection", "module", False),
        ("convert_to_error", "function", True),
        ("dispatch", "function", True),
        ("format_exc", "function", True),
        ("get_context", "method", True),
        ("get_spawning_popen", "function", True),
        ("getpid", "builtin_function_or_method", True),
        ("listener_client", "dict", False),
        ("os", "module", False),
        ("pool", "module", False),
        ("process", "module", False),
        ("public_methods", "function", True),
        ("queue", "module", False),
        ("rebuild_as_list", "function", True),
        ("reduce_array", "function", True),
        ("reduction", "module", False),
        ("shared_memory", "module", False),
        ("signal", "module", False),
        ("sys", "module", False),
        ("threading", "module", False),
        ("time", "module", False),
        ("types", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.pool": [
        ("ApplyResult", "type", True),
        ("AsyncResult", "type", True),
        ("CLOSE", "str", False),
        ("ExceptionWithTraceback", "type", True),
        ("IMapIterator", "type", True),
        ("IMapUnorderedIterator", "type", True),
        ("INIT", "str", False),
        ("MapResult", "type", True),
        ("MaybeEncodingError", "type", True),
        ("Pool", "type", True),
        ("RUN", "str", False),
        ("RemoteTraceback", "type", True),
        ("TERMINATE", "str", False),
        ("ThreadPool", "type", True),
        ("TimeoutError", "type", True),
        ("collections", "module", False),
        ("get_context", "method", True),
        ("itertools", "module", False),
        ("job_counter", "count", False),
        ("mapstar", "function", True),
        ("os", "module", False),
        ("queue", "module", False),
        ("rebuild_exc", "function", True),
        ("starmapstar", "function", True),
        ("threading", "module", False),
        ("time", "module", False),
        ("traceback", "module", False),
        ("types", "module", False),
        ("util", "module", False),
        ("wait", "function", True),
        ("warnings", "module", False),
        ("worker", "function", True),
    ],
    "multiprocessing.popen_fork": [
        ("Popen", "type", True),
        ("os", "module", False),
        ("signal", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.popen_forkserver": [
        ("Popen", "type", True),
        ("forkserver", "module", False),
        ("io", "module", False),
        ("os", "module", False),
        ("popen_fork", "module", False),
        ("reduction", "module", False),
        ("set_spawning_popen", "function", True),
        ("spawn", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.popen_spawn_posix": [
        ("Popen", "type", True),
        ("io", "module", False),
        ("os", "module", False),
        ("popen_fork", "module", False),
        ("reduction", "module", False),
        ("set_spawning_popen", "function", True),
        ("spawn", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.process": [
        ("AuthenticationString", "type", True),
        ("BaseProcess", "type", True),
        ("ORIGINAL_DIR", "str", False),
        ("WeakSet", "type", True),
        ("active_children", "function", True),
        ("current_process", "function", True),
        ("itertools", "module", False),
        ("os", "module", False),
        ("parent_process", "function", True),
        ("signal", "module", False),
        ("sys", "module", False),
        ("threading", "module", False),
    ],
    "multiprocessing.queues": [
        ("Empty", "type", True),
        ("Finalize", "type", True),
        ("Full", "type", True),
        ("JoinableQueue", "type", True),
        ("Queue", "type", True),
        ("SimpleQueue", "type", True),
        ("collections", "module", False),
        ("connection", "module", False),
        ("context", "module", False),
        ("debug", "function", True),
        ("errno", "module", False),
        ("info", "function", True),
        ("is_exiting", "function", True),
        ("os", "module", False),
        ("register_after_fork", "function", True),
        ("sys", "module", False),
        ("threading", "module", False),
        ("time", "module", False),
        ("types", "module", False),
        ("weakref", "module", False),
    ],
    "multiprocessing.reduction": [
        ("ABCMeta", "type", True),
        ("ACKNOWLEDGE", "bool", False),
        ("AbstractReducer", "ABCMeta", True),
        ("DupFd", "function", True),
        ("ForkingPickler", "type", True),
        ("HAVE_SEND_HANDLE", "bool", False),
        ("array", "module", False),
        ("context", "module", False),
        ("copyreg", "module", False),
        ("dump", "function", True),
        ("functools", "module", False),
        ("io", "module", False),
        ("os", "module", False),
        ("pickle", "module", False),
        ("recv_handle", "function", True),
        ("recvfds", "function", True),
        ("register", "method", True),
        ("send_handle", "function", True),
        ("sendfds", "function", True),
        ("socket", "module", False),
        ("sys", "module", False),
    ],
    "multiprocessing.resource_sharer": [
        ("DupFd", "type", True),
        ("os", "module", False),
        ("process", "module", False),
        ("reduction", "module", False),
        ("signal", "module", False),
        ("socket", "module", False),
        ("stop", "method", True),
        ("sys", "module", False),
        ("threading", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.resource_tracker": [
        ("ReentrantCallError", "type", True),
        ("ResourceTracker", "type", True),
        ("ensure_running", "method", True),
        ("getfd", "method", True),
        ("main", "function", True),
        ("os", "module", False),
        ("register", "method", True),
        ("signal", "module", False),
        ("spawn", "module", False),
        ("sys", "module", False),
        ("threading", "module", False),
        ("unregister", "method", True),
        ("util", "module", False),
        ("warnings", "module", False),
    ],
    "multiprocessing.shared_memory": [
        ("ShareableList", "type", True),
        ("SharedMemory", "type", True),
        ("errno", "module", False),
        ("mmap", "module", False),
        ("os", "module", False),
        ("partial", "type", True),
        ("resource_tracker", "module", False),
        ("secrets", "module", False),
        ("struct", "module", False),
        ("types", "module", False),
    ],
    "multiprocessing.sharedctypes": [
        ("Array", "function", True),
        ("RawArray", "function", True),
        ("RawValue", "function", True),
        ("Synchronized", "type", True),
        ("SynchronizedArray", "type", True),
        ("SynchronizedBase", "type", True),
        ("SynchronizedString", "type", True),
        ("Value", "function", True),
        ("assert_spawning", "function", True),
        ("class_cache", "WeakKeyDictionary", False),
        ("copy", "function", True),
        ("ctypes", "module", False),
        ("get_context", "method", True),
        ("heap", "module", False),
        ("make_property", "function", True),
        ("prop_cache", "dict", False),
        ("rebuild_ctype", "function", True),
        ("reduce_ctype", "function", True),
        ("reduction", "module", False),
        ("synchronized", "function", True),
        ("template", "str", False),
        ("typecode_to_type", "dict", False),
        ("weakref", "module", False),
    ],
    "multiprocessing.spawn": [
        ("WINEXE", "bool", False),
        ("WINSERVICE", "bool", False),
        ("freeze_support", "function", True),
        ("get_command_line", "function", True),
        ("get_executable", "function", True),
        ("get_preparation_data", "function", True),
        ("get_start_method", "method", True),
        ("import_main_path", "function", True),
        ("is_forking", "function", True),
        ("old_main_modules", "list", False),
        ("os", "module", False),
        ("prepare", "function", True),
        ("process", "module", False),
        ("reduction", "module", False),
        ("runpy", "module", False),
        ("set_executable", "function", True),
        ("set_start_method", "method", True),
        ("spawn_main", "function", True),
        ("sys", "module", False),
        ("types", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.synchronize": [
        ("Barrier", "type", True),
        ("BoundedSemaphore", "type", True),
        ("Condition", "type", True),
        ("Event", "type", True),
        ("Lock", "type", True),
        ("RECURSIVE_MUTEX", "int", False),
        ("RLock", "type", True),
        ("SEMAPHORE", "int", False),
        ("SEM_VALUE_MAX", "int", False),
        ("SemLock", "type", True),
        ("Semaphore", "type", True),
        ("context", "module", False),
        ("process", "module", False),
        ("sem_unlink", "builtin_function_or_method", True),
        ("sys", "module", False),
        ("tempfile", "module", False),
        ("threading", "module", False),
        ("time", "module", False),
        ("util", "module", False),
    ],
    "multiprocessing.util": [
        ("DEBUG", "int", False),
        ("DEFAULT_LOGGING_FORMAT", "str", False),
        ("Finalize", "type", True),
        ("ForkAwareLocal", "type", True),
        ("ForkAwareThreadLock", "type", True),
        ("INFO", "int", False),
        ("LOGGER_NAME", "str", False),
        ("MAXFD", "int", False),
        ("NOTSET", "int", False),
        ("SUBDEBUG", "int", False),
        ("SUBWARNING", "int", False),
        ("abstract_sockets_supported", "bool", False),
        ("atexit", "module", False),
        ("close_all_fds_except", "function", True),
        ("close_fds", "function", True),
        ("debug", "function", True),
        ("get_logger", "function", True),
        ("get_temp_dir", "function", True),
        ("info", "function", True),
        ("is_abstract_socket_namespace", "function", True),
        ("is_exiting", "function", True),
        ("itertools", "module", False),
        ("log_to_stderr", "function", True),
        ("os", "module", False),
        ("process", "module", False),
        ("register_after_fork", "function", True),
        ("spawnv_passfds", "function", True),
        ("sub_debug", "function", True),
        ("sub_warning", "function", True),
        ("sys", "module", False),
        ("threading", "module", False),
        ("weakref", "module", False),
    ],
}

globals().pop("_require_intrinsic", None)
