"""Importable builtins for Molt.

Bind supported builtins to module globals so `import builtins` works in
compiled code without introducing dynamic indirection.
"""

from __future__ import annotations

import builtins as _py_builtins

from molt import intrinsics as _intrinsics

TYPE_CHECKING = False

_ORIG_COMPILE = getattr(_py_builtins, "compile", None)


def cast(_type, value):
    return value


if TYPE_CHECKING:
    from typing import Any, Callable, Optional
else:

    class _TypingAlias:
        __slots__ = ()

        def __getitem__(self, _item):
            return self

    Any = object()
    Callable = _TypingAlias()
    Optional = _TypingAlias()

_builtin_property = getattr(_py_builtins, "property", None)
if _builtin_property is None or _builtin_property is object:

    class _PropertyProbe:
        @property
        def value(self):
            return None

    property = type(_PropertyProbe.value)
else:
    property = _builtin_property

try:
    import _molt_importer as _molt_importer

    if hasattr(_molt_importer, "_molt_import"):
        __import__ = _molt_importer._molt_import
except Exception:
    pass

if TYPE_CHECKING:
    _molt_getargv: Callable[[], list[str]]
    _molt_getframe: Callable[[object], object]
    _molt_getrecursionlimit: Callable[[], int]
    _molt_setrecursionlimit: Callable[[int], None]
    _molt_sys_version_info: Callable[[], tuple[int, int, int, str, int]]
    _molt_sys_version: Callable[[], str]
    _molt_sys_stdin: Callable[[], object]
    _molt_sys_stdout: Callable[[], object]
    _molt_sys_stderr: Callable[[], object]
    _molt_sys_executable: Callable[[], str]
    _molt_exception_last: Callable[[], Optional[BaseException]]
    _molt_exception_active: Callable[[], Optional[BaseException]]
    _molt_asyncgen_hooks_get: Callable[[], object]
    _molt_asyncgen_hooks_set: Callable[[object, object], object]
    _molt_asyncgen_locals: Callable[[object], object]
    _molt_gen_locals: Callable[[object], object]
    _molt_code_new: Callable[[object, object, object, object], object]
    molt_compile_builtin: Callable[[object, object, object, int, bool, int], object]
    _molt_module_new: Callable[[object], object]
    _molt_function_set_builtin: Callable[[object], object]
    _molt_class_new: Callable[[object], object]
    _molt_class_set_base: Callable[[object, object], object]
    _molt_class_apply_set_name: Callable[[object], object]
    _molt_os_name: Callable[[], str]
    _molt_sys_platform: Callable[[], str]
    _molt_time_monotonic: Callable[[], float]
    _molt_time_monotonic_ns: Callable[[], int]
    _molt_time_time: Callable[[], float]
    _molt_time_time_ns: Callable[[], int]
    _molt_getpid: Callable[[], int]
    _molt_getcwd: Callable[[], str]
    _molt_os_close: Callable[[object], object]
    _molt_os_dup: Callable[[object], object]
    _molt_os_get_inheritable: Callable[[object], object]
    _molt_os_set_inheritable: Callable[[object, object], object]
    _molt_struct_pack: Callable[[object, object], object]
    _molt_struct_unpack: Callable[[object, object], object]
    _molt_struct_calcsize: Callable[[object], object]
    _molt_env_get_raw: Callable[..., object]
    _molt_env_snapshot: Callable[[], object]
    _molt_errno_constants: Callable[[], tuple[dict[str, int], dict[int, str]]]
    _molt_path_exists: Callable[[object], bool]
    _molt_path_listdir: Callable[[object], object]
    _molt_path_mkdir: Callable[[object], object]
    _molt_path_unlink: Callable[[object], None]
    _molt_path_rmdir: Callable[[object], None]
    _molt_path_chmod: Callable[[object, object], None]
    _molt_io_wait_new: Callable[[object, int, object], object]
    molt_block_on: Callable[[object], object]
    molt_asyncgen_shutdown: Callable[[], object]
    molt_db_query_obj: Callable[[object, object], object]
    molt_db_exec_obj: Callable[[object, object], object]
    molt_msgpack_parse_scalar_obj: Callable[[object], object]
    molt_weakref_register: Callable[[object, object, object], object]
    molt_weakref_get: Callable[[object], object]
    molt_weakref_drop: Callable[[object], object]
    molt_thread_spawn: Callable[[object], object]
    molt_thread_join: Callable[[object, object], object]
    molt_thread_is_alive: Callable[[object], object]
    molt_thread_ident: Callable[[object], object]
    molt_thread_native_id: Callable[[object], object]
    molt_thread_current_ident: Callable[[], object]
    molt_thread_current_native_id: Callable[[], object]
    molt_thread_drop: Callable[[object], object]
    molt_chan_new: Callable[[object], object]
    molt_chan_send: Callable[[object, object], object]
    molt_chan_recv: Callable[[object], object]
    molt_chan_try_send: Callable[[object, object], object]
    molt_chan_try_recv: Callable[[object], object]
    molt_chan_send_blocking: Callable[[object, object], object]
    molt_chan_recv_blocking: Callable[[object], object]
    molt_chan_drop: Callable[[object], object]
    molt_lock_new: Callable[[], object]
    molt_lock_acquire: Callable[[object, object, object], object]
    molt_lock_release: Callable[[object], object]
    molt_lock_locked: Callable[[object], object]
    molt_lock_drop: Callable[[object], object]
    molt_rlock_new: Callable[[], object]
    molt_rlock_acquire: Callable[[object, object, object], object]
    molt_rlock_release: Callable[[object], object]
    molt_rlock_locked: Callable[[object], object]
    molt_rlock_drop: Callable[[object], object]

_INTRINSIC_NAMES = [
    "_molt_getargv",
    "_molt_getframe",
    "_molt_getrecursionlimit",
    "_molt_setrecursionlimit",
    "_molt_sys_version_info",
    "_molt_sys_version",
    "_molt_sys_stdin",
    "_molt_sys_stdout",
    "_molt_sys_stderr",
    "_molt_sys_executable",
    "_molt_exception_last",
    "_molt_exception_active",
    "_molt_asyncgen_hooks_get",
    "_molt_asyncgen_hooks_set",
    "_molt_asyncgen_locals",
    "_molt_gen_locals",
    "_molt_code_new",
    "molt_compile_builtin",
    "_molt_module_new",
    "_molt_function_set_builtin",
    "_molt_class_new",
    "_molt_class_set_base",
    "_molt_class_apply_set_name",
    "_molt_os_name",
    "_molt_sys_platform",
    "_molt_time_monotonic",
    "_molt_time_monotonic_ns",
    "_molt_time_time",
    "_molt_time_time_ns",
    "_molt_getpid",
    "_molt_getcwd",
    "_molt_os_close",
    "_molt_os_dup",
    "_molt_os_get_inheritable",
    "_molt_os_set_inheritable",
    "_molt_struct_pack",
    "_molt_struct_unpack",
    "_molt_struct_calcsize",
    "_molt_env_get_raw",
    "_molt_env_snapshot",
    "_molt_errno_constants",
    "_molt_path_exists",
    "_molt_path_listdir",
    "_molt_path_mkdir",
    "_molt_path_unlink",
    "_molt_path_rmdir",
    "_molt_path_chmod",
    "_molt_io_wait_new",
    "molt_block_on",
    "molt_asyncgen_shutdown",
    "molt_process_spawn",
    "molt_process_wait_future",
    "molt_process_pid",
    "molt_process_returncode",
    "molt_process_kill",
    "molt_process_terminate",
    "molt_process_stdin",
    "molt_process_stdout",
    "molt_process_stderr",
    "molt_process_drop",
    "molt_thread_spawn",
    "molt_thread_join",
    "molt_thread_is_alive",
    "molt_thread_ident",
    "molt_thread_native_id",
    "molt_thread_current_ident",
    "molt_thread_current_native_id",
    "molt_thread_drop",
    "molt_chan_new",
    "molt_chan_send",
    "molt_chan_recv",
    "molt_chan_try_send",
    "molt_chan_try_recv",
    "molt_chan_send_blocking",
    "molt_chan_recv_blocking",
    "molt_chan_drop",
    "molt_pending",
    "molt_lock_new",
    "molt_lock_acquire",
    "molt_lock_release",
    "molt_lock_locked",
    "molt_lock_drop",
    "molt_rlock_new",
    "molt_rlock_acquire",
    "molt_rlock_release",
    "molt_rlock_locked",
    "molt_rlock_drop",
    "molt_stream_new",
    "molt_stream_clone",
    "molt_stream_send_obj",
    "molt_stream_recv",
    "molt_stream_close",
    "molt_stream_drop",
    "molt_db_query_obj",
    "molt_db_exec_obj",
    "molt_msgpack_parse_scalar_obj",
    "molt_weakref_register",
    "molt_weakref_get",
    "molt_weakref_drop",
    "molt_module_cache_set",
]


def _install_intrinsic(name: str, value: Any) -> None:
    if value is None:
        try:
            existing = getattr(_py_builtins, name)
        except Exception:
            return
        if existing is None:
            return
        _intrinsics.register(name, existing)
        return
    _intrinsics.register(name, value)


# Force builtin_func emission for intrinsics referenced by string lookups while
# avoiding module-scope assignments that would predeclare globals to None.
try:
    _install_intrinsic("_molt_io_wait_new", _molt_io_wait_new)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_io_wait_new", None)
try:
    _install_intrinsic("molt_block_on", molt_block_on)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_block_on", None)
try:
    _install_intrinsic("molt_asyncgen_shutdown", molt_asyncgen_shutdown)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_asyncgen_shutdown", None)
try:
    _install_intrinsic("molt_pending", molt_pending)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_pending", None)
try:
    _install_intrinsic("_molt_asyncgen_hooks_get", _molt_asyncgen_hooks_get)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_asyncgen_hooks_get", None)
try:
    _install_intrinsic("_molt_asyncgen_hooks_set", _molt_asyncgen_hooks_set)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_asyncgen_hooks_set", None)
try:
    _install_intrinsic("_molt_asyncgen_locals", _molt_asyncgen_locals)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_asyncgen_locals", None)
try:
    _install_intrinsic("_molt_gen_locals", _molt_gen_locals)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_gen_locals", None)
try:
    _install_intrinsic("_molt_code_new", _molt_code_new)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_code_new", None)
try:
    _install_intrinsic("molt_compile_builtin", molt_compile_builtin)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_compile_builtin", None)


def compile(
    source: object,
    filename: object,
    mode: object,
    flags: int = 0,
    dont_inherit: bool = False,
    optimize: int = -1,
):
    intrinsic = _load_intrinsic("molt_compile_builtin")
    if callable(intrinsic):
        return intrinsic(source, filename, mode, flags, dont_inherit, optimize)
    compile_func = _ORIG_COMPILE
    if compile_func is None or compile_func is compile:
        raise NotImplementedError("compile() intrinsic unavailable")
    compile_func = cast(
        Callable[[object, object, object, int, bool, int], object],
        compile_func,
    )
    return compile_func(source, filename, mode, flags, dont_inherit, optimize)


try:
    _install_intrinsic("_molt_sys_executable", _molt_sys_executable)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_sys_executable", None)
try:
    _install_intrinsic("molt_sys_set_version_info", molt_sys_set_version_info)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_sys_set_version_info", None)
try:
    _install_intrinsic("_molt_getargv", _molt_getargv)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_getargv", None)
try:
    _install_intrinsic("_molt_getframe", _molt_getframe)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_getframe", None)
try:
    _install_intrinsic("_molt_getrecursionlimit", _molt_getrecursionlimit)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_getrecursionlimit", None)
try:
    _install_intrinsic("_molt_setrecursionlimit", _molt_setrecursionlimit)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_setrecursionlimit", None)
try:
    _install_intrinsic("_molt_sys_version_info", _molt_sys_version_info)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_sys_version_info", None)
try:
    _install_intrinsic("_molt_sys_version", _molt_sys_version)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_sys_version", None)
try:
    _install_intrinsic("_molt_sys_stdin", _molt_sys_stdin)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_sys_stdin", None)
try:
    _install_intrinsic("_molt_sys_stdout", _molt_sys_stdout)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_sys_stdout", None)
try:
    _install_intrinsic("_molt_sys_stderr", _molt_sys_stderr)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_sys_stderr", None)
try:
    _install_intrinsic("_molt_env_get_raw", _molt_env_get_raw)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_env_get_raw", None)
try:
    _install_intrinsic("_molt_env_snapshot", _molt_env_snapshot)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_env_snapshot", None)
try:
    _install_intrinsic("_molt_path_exists", _molt_path_exists)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_path_exists", None)
try:
    _install_intrinsic("_molt_path_listdir", _molt_path_listdir)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_path_listdir", None)
try:
    _install_intrinsic("_molt_path_mkdir", _molt_path_mkdir)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_path_mkdir", None)
try:
    _install_intrinsic("_molt_path_unlink", _molt_path_unlink)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_path_unlink", None)
try:
    _install_intrinsic("_molt_path_rmdir", _molt_path_rmdir)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_path_rmdir", None)
try:
    _install_intrinsic("_molt_path_chmod", _molt_path_chmod)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_path_chmod", None)
try:
    _install_intrinsic("_molt_os_close", _molt_os_close)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_os_close", None)
try:
    _install_intrinsic("_molt_os_dup", _molt_os_dup)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_os_dup", None)
try:
    _install_intrinsic(
        "_molt_os_get_inheritable",
        _molt_os_get_inheritable,  # type: ignore[name-defined]  # noqa: F821
    )
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_os_get_inheritable", None)
try:
    _install_intrinsic(
        "_molt_os_set_inheritable",
        _molt_os_set_inheritable,  # type: ignore[name-defined]  # noqa: F821
    )
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_os_set_inheritable", None)
try:
    _install_intrinsic("_molt_struct_pack", _molt_struct_pack)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_struct_pack", None)
try:
    _install_intrinsic("_molt_struct_unpack", _molt_struct_unpack)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_struct_unpack", None)
try:
    _install_intrinsic("_molt_struct_calcsize", _molt_struct_calcsize)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_struct_calcsize", None)
try:
    _install_intrinsic("_molt_socket_constants", _molt_socket_constants)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_constants", None)
try:
    _install_intrinsic("_molt_socket_has_ipv6", _molt_socket_has_ipv6)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_has_ipv6", None)
try:
    _install_intrinsic("_molt_socket_new", _molt_socket_new)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_new", None)
try:
    _install_intrinsic("_molt_socket_close", _molt_socket_close)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_close", None)
try:
    _install_intrinsic("_molt_socket_drop", _molt_socket_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_drop", None)
try:
    _install_intrinsic("_molt_socket_clone", _molt_socket_clone)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_clone", None)
try:
    _install_intrinsic("_molt_socket_fileno", _molt_socket_fileno)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_fileno", None)
try:
    _install_intrinsic("_molt_socket_gettimeout", _molt_socket_gettimeout)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_gettimeout", None)
try:
    _install_intrinsic("_molt_socket_settimeout", _molt_socket_settimeout)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_settimeout", None)
try:
    _install_intrinsic("_molt_socket_setblocking", _molt_socket_setblocking)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_setblocking", None)
try:
    _install_intrinsic("_molt_socket_getblocking", _molt_socket_getblocking)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getblocking", None)
try:
    _install_intrinsic("_molt_socket_bind", _molt_socket_bind)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_bind", None)
try:
    _install_intrinsic("_molt_socket_listen", _molt_socket_listen)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_listen", None)
try:
    _install_intrinsic("_molt_socket_accept", _molt_socket_accept)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_accept", None)
try:
    _install_intrinsic("_molt_socket_connect", _molt_socket_connect)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_connect", None)
try:
    _install_intrinsic("_molt_socket_connect_ex", _molt_socket_connect_ex)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_connect_ex", None)
try:
    _install_intrinsic("_molt_socket_recv", _molt_socket_recv)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_recv", None)
try:
    _install_intrinsic("_molt_socket_recv_into", _molt_socket_recv_into)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_recv_into", None)
try:
    _install_intrinsic("_molt_socket_send", _molt_socket_send)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_send", None)
try:
    _install_intrinsic("_molt_socket_sendall", _molt_socket_sendall)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_sendall", None)
try:
    _install_intrinsic("_molt_socket_sendto", _molt_socket_sendto)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_sendto", None)
try:
    _install_intrinsic("_molt_socket_recvfrom", _molt_socket_recvfrom)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_recvfrom", None)
try:
    _install_intrinsic("_molt_socket_shutdown", _molt_socket_shutdown)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_shutdown", None)
try:
    _install_intrinsic("_molt_socket_getsockname", _molt_socket_getsockname)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getsockname", None)
try:
    _install_intrinsic("_molt_socket_getpeername", _molt_socket_getpeername)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getpeername", None)
try:
    _install_intrinsic("_molt_socket_setsockopt", _molt_socket_setsockopt)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_setsockopt", None)
try:
    _install_intrinsic("_molt_socket_getsockopt", _molt_socket_getsockopt)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getsockopt", None)
try:
    _install_intrinsic("_molt_socket_detach", _molt_socket_detach)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_detach", None)
try:
    _install_intrinsic("_molt_socketpair", _molt_socketpair)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socketpair", None)
try:
    _install_intrinsic("_molt_socket_getaddrinfo", _molt_socket_getaddrinfo)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getaddrinfo", None)
try:
    _install_intrinsic("_molt_socket_getnameinfo", _molt_socket_getnameinfo)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getnameinfo", None)
try:
    _install_intrinsic("_molt_socket_gethostname", _molt_socket_gethostname)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_gethostname", None)
try:
    _install_intrinsic("_molt_socket_getservbyname", _molt_socket_getservbyname)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getservbyname", None)
try:
    _install_intrinsic("_molt_socket_getservbyport", _molt_socket_getservbyport)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_getservbyport", None)
try:
    _install_intrinsic("_molt_socket_inet_pton", _molt_socket_inet_pton)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_inet_pton", None)
try:
    _install_intrinsic("_molt_socket_inet_ntop", _molt_socket_inet_ntop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("_molt_socket_inet_ntop", None)
try:
    _install_intrinsic("molt_process_spawn", molt_process_spawn)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_spawn", None)
try:
    _install_intrinsic("molt_process_wait_future", molt_process_wait_future)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_wait_future", None)
try:
    _install_intrinsic("molt_process_pid", molt_process_pid)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_pid", None)
try:
    _install_intrinsic("molt_process_returncode", molt_process_returncode)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_returncode", None)
try:
    _install_intrinsic("molt_process_kill", molt_process_kill)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_kill", None)
try:
    _install_intrinsic("molt_process_terminate", molt_process_terminate)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_terminate", None)
try:
    _install_intrinsic("molt_process_stdin", molt_process_stdin)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_stdin", None)
try:
    _install_intrinsic("molt_process_stdout", molt_process_stdout)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_stdout", None)
try:
    _install_intrinsic("molt_process_stderr", molt_process_stderr)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_stderr", None)
try:
    _install_intrinsic("molt_process_drop", molt_process_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_process_drop", None)
try:
    _install_intrinsic("molt_thread_spawn", molt_thread_spawn)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_spawn", None)
try:
    _install_intrinsic("molt_thread_join", molt_thread_join)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_join", None)
try:
    _install_intrinsic("molt_thread_is_alive", molt_thread_is_alive)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_is_alive", None)
try:
    _install_intrinsic("molt_thread_ident", molt_thread_ident)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_ident", None)
try:
    _install_intrinsic("molt_thread_native_id", molt_thread_native_id)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_native_id", None)
try:
    _install_intrinsic("molt_thread_current_ident", molt_thread_current_ident)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_current_ident", None)
try:
    _install_intrinsic("molt_thread_current_native_id", molt_thread_current_native_id)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_current_native_id", None)
try:
    _install_intrinsic("molt_thread_drop", molt_thread_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_thread_drop", None)
try:
    _install_intrinsic("molt_chan_new", molt_chan_new)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_new", None)
try:
    _install_intrinsic("molt_chan_send", molt_chan_send)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_send", None)
try:
    _install_intrinsic("molt_chan_recv", molt_chan_recv)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_recv", None)
try:
    _install_intrinsic("molt_chan_try_send", molt_chan_try_send)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_try_send", None)
try:
    _install_intrinsic("molt_chan_try_recv", molt_chan_try_recv)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_try_recv", None)
try:
    _install_intrinsic(
        "molt_chan_send_blocking",
        molt_chan_send_blocking,  # type: ignore[name-defined]  # noqa: F821
    )
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_send_blocking", None)
try:
    _install_intrinsic(
        "molt_chan_recv_blocking",
        molt_chan_recv_blocking,  # type: ignore[name-defined]  # noqa: F821
    )
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_recv_blocking", None)
try:
    _install_intrinsic("molt_chan_drop", molt_chan_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_chan_drop", None)
try:
    _install_intrinsic("molt_module_cache_set", molt_module_cache_set)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_module_cache_set", None)
try:
    _install_intrinsic("molt_lock_new", molt_lock_new)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_lock_new", None)
try:
    _install_intrinsic("molt_lock_acquire", molt_lock_acquire)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_lock_acquire", None)
try:
    _install_intrinsic("molt_lock_release", molt_lock_release)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_lock_release", None)
try:
    _install_intrinsic("molt_lock_locked", molt_lock_locked)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_lock_locked", None)
try:
    _install_intrinsic("molt_lock_drop", molt_lock_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_lock_drop", None)
try:
    _install_intrinsic("molt_rlock_new", molt_rlock_new)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_rlock_new", None)
try:
    _install_intrinsic("molt_rlock_acquire", molt_rlock_acquire)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_rlock_acquire", None)
try:
    _install_intrinsic("molt_rlock_release", molt_rlock_release)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_rlock_release", None)
try:
    _install_intrinsic("molt_rlock_locked", molt_rlock_locked)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_rlock_locked", None)
try:
    _install_intrinsic("molt_rlock_drop", molt_rlock_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_rlock_drop", None)
try:
    _install_intrinsic("molt_stream_new", molt_stream_new)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_stream_new", None)
try:
    _install_intrinsic("molt_stream_clone", molt_stream_clone)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_stream_clone", None)
try:
    _install_intrinsic("molt_stream_send_obj", molt_stream_send_obj)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_stream_send_obj", None)
try:
    _install_intrinsic("molt_stream_recv", molt_stream_recv)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_stream_recv", None)
try:
    _install_intrinsic("molt_stream_close", molt_stream_close)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_stream_close", None)
try:
    _install_intrinsic("molt_stream_drop", molt_stream_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_stream_drop", None)
try:
    _install_intrinsic("molt_db_query_obj", molt_db_query_obj)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_db_query_obj", None)
try:
    _install_intrinsic("molt_db_exec_obj", molt_db_exec_obj)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_db_exec_obj", None)
try:
    _install_intrinsic("molt_msgpack_parse_scalar_obj", molt_msgpack_parse_scalar_obj)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_msgpack_parse_scalar_obj", None)
try:
    _install_intrinsic("molt_weakref_register", molt_weakref_register)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_weakref_register", None)
try:
    _install_intrinsic("molt_weakref_get", molt_weakref_get)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_weakref_get", None)
try:
    _install_intrinsic("molt_weakref_drop", molt_weakref_drop)  # type: ignore[name-defined]  # noqa: F821
except NameError:  # pragma: no cover - absent in host CPython
    _install_intrinsic("molt_weakref_drop", None)


def _load_intrinsic(name: str) -> Any | None:
    return _intrinsics.load(name, globals())


_intrinsics.register_from_builtins(_INTRINSIC_NAMES)


# TODO(type-coverage, owner:stdlib, milestone:TC3, priority:P2, status:missing): implement eval/exec builtins with sandboxing rules.
__all__ = [
    "object",
    "type",
    "isinstance",
    "issubclass",
    "len",
    "hash",
    "ord",
    "chr",
    "ascii",
    "bin",
    "oct",
    "hex",
    "abs",
    "divmod",
    "repr",
    "format",
    "dir",
    "callable",
    "any",
    "all",
    "sum",
    "sorted",
    "min",
    "max",
    "id",
    "str",
    "range",
    "enumerate",
    "slice",
    "list",
    "tuple",
    "dict",
    "float",
    "complex",
    "int",
    "bool",
    "round",
    "set",
    "frozenset",
    "bytes",
    "bytearray",
    "memoryview",
    "iter",
    "map",
    "filter",
    "zip",
    "reversed",
    "next",
    "aiter",
    "anext",
    "getattr",
    "setattr",
    "delattr",
    "hasattr",
    "super",
    "print",
    "vars",
    "Ellipsis",
    "NotImplemented",
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
    "CancelledError",
    "ProcessLookupError",
    "TypeError",
    "UnboundLocalError",
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
]

object = object

type = type

isinstance = isinstance
issubclass = issubclass

len = len
hash = hash
ord = ord
chr = chr
ascii = ascii
bin = bin
oct = oct
hex = hex
abs = abs
divmod = divmod
repr = repr
format = format
dir = dir
callable = callable
any = any
all = all
sum = sum
sorted = sorted
min = min
max = max
id = id
str = str
range = range
enumerate = enumerate
slice = slice
list = list
tuple = tuple
dict = dict
float = float
try:
    complex = complex
except NameError:
    try:
        complex = type(0j)
    except Exception:
        pass
int = int
bool = bool
round = round
set = set
frozenset = frozenset
bytes = bytes
bytearray = bytearray
memoryview = memoryview
iter = iter
map = map
filter = filter
zip = zip
reversed = reversed
next = next
aiter = aiter
anext = anext
getattr = getattr
setattr = setattr
delattr = delattr
hasattr = hasattr
super = super
print = print
vars = vars
Ellipsis = ...
NotImplemented = NotImplemented
BaseException = BaseException
BaseExceptionGroup = BaseExceptionGroup
Exception = Exception
ExceptionGroup = ExceptionGroup
try:
    _cancelled_error = getattr(_py_builtins, "CancelledError")
except AttributeError:
    _cancelled_error = None

if _cancelled_error is None:

    class CancelledError(_py_builtins.BaseException):
        pass

else:
    CancelledError = _cancelled_error


ArithmeticError = ArithmeticError
AssertionError = AssertionError
AttributeError = AttributeError
BufferError = BufferError
EOFError = EOFError
FloatingPointError = FloatingPointError
GeneratorExit = GeneratorExit
ImportError = ImportError
ModuleNotFoundError = ModuleNotFoundError
IndexError = IndexError
KeyError = KeyError
KeyboardInterrupt = KeyboardInterrupt
LookupError = LookupError
MemoryError = MemoryError
NameError = NameError
UnboundLocalError = UnboundLocalError
NotImplementedError = NotImplementedError
OSError = OSError
EnvironmentError = EnvironmentError
IOError = IOError
BlockingIOError = BlockingIOError
ChildProcessError = ChildProcessError
ConnectionError = ConnectionError
BrokenPipeError = BrokenPipeError
ConnectionAbortedError = ConnectionAbortedError
ConnectionRefusedError = ConnectionRefusedError
ConnectionResetError = ConnectionResetError
FileExistsError = FileExistsError
OverflowError = OverflowError
PermissionError = PermissionError
FileNotFoundError = FileNotFoundError
InterruptedError = InterruptedError
IsADirectoryError = IsADirectoryError
NotADirectoryError = NotADirectoryError
RecursionError = RecursionError
ReferenceError = ReferenceError
RuntimeError = RuntimeError
StopIteration = StopIteration
StopAsyncIteration = StopAsyncIteration
SyntaxError = SyntaxError
IndentationError = IndentationError
TabError = TabError
SystemError = SystemError
SystemExit = SystemExit
TimeoutError = TimeoutError
ProcessLookupError = ProcessLookupError
TypeError = TypeError
UnicodeError = UnicodeError
UnicodeDecodeError = UnicodeDecodeError
UnicodeEncodeError = UnicodeEncodeError
UnicodeTranslateError = UnicodeTranslateError
ValueError = ValueError
ZeroDivisionError = ZeroDivisionError
Warning = Warning
DeprecationWarning = DeprecationWarning
PendingDeprecationWarning = PendingDeprecationWarning
RuntimeWarning = RuntimeWarning
SyntaxWarning = SyntaxWarning
UserWarning = UserWarning
FutureWarning = FutureWarning
ImportWarning = ImportWarning
UnicodeWarning = UnicodeWarning
BytesWarning = BytesWarning
ResourceWarning = ResourceWarning
EncodingWarning = EncodingWarning

WindowsError = getattr(_py_builtins, "WindowsError", OSError)

_molt_getargv = cast(Callable[[], list[str]], _load_intrinsic("_molt_getargv"))
_molt_getframe = cast(Callable[[object], object], _load_intrinsic("_molt_getframe"))
_molt_getrecursionlimit = cast(
    Callable[[], int],
    _load_intrinsic("_molt_getrecursionlimit"),
)
_molt_setrecursionlimit = cast(
    Callable[[int], None],
    _load_intrinsic("_molt_setrecursionlimit"),
)
_molt_sys_version_info = cast(
    Callable[[], tuple[int, int, int, str, int]],
    _load_intrinsic("_molt_sys_version_info"),
)
_molt_sys_version = cast(Callable[[], str], _load_intrinsic("_molt_sys_version"))
_molt_sys_stdin = cast(Callable[[], object], _load_intrinsic("_molt_sys_stdin"))
_molt_sys_stdout = cast(Callable[[], object], _load_intrinsic("_molt_sys_stdout"))
_molt_sys_stderr = cast(Callable[[], object], _load_intrinsic("_molt_sys_stderr"))
_molt_exception_last = cast(
    Callable[[], Optional[BaseException]],
    _load_intrinsic("_molt_exception_last"),
)
_molt_exception_active = cast(
    Callable[[], Optional[BaseException]],
    _load_intrinsic("_molt_exception_active"),
)
_molt_asyncgen_hooks_get = cast(
    Callable[[], object],
    _load_intrinsic("_molt_asyncgen_hooks_get"),
)
_molt_asyncgen_hooks_set = cast(
    Callable[[object, object], object],
    _load_intrinsic("_molt_asyncgen_hooks_set"),
)
_molt_asyncgen_locals = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_asyncgen_locals"),
)
_molt_module_new = cast(Callable[[object], object], _load_intrinsic("_molt_module_new"))
_molt_function_set_builtin = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_function_set_builtin"),
)
_molt_class_new = cast(Callable[[object], object], _load_intrinsic("_molt_class_new"))
_molt_class_set_base = cast(
    Callable[[object, object], object],
    _load_intrinsic("_molt_class_set_base"),
)
_molt_class_apply_set_name = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_class_apply_set_name"),
)
_molt_os_name = cast(Callable[[], str], _load_intrinsic("_molt_os_name"))
_molt_sys_platform = cast(Callable[[], str], _load_intrinsic("_molt_sys_platform"))
_molt_time_monotonic = cast(
    Callable[[], float], _load_intrinsic("_molt_time_monotonic")
)
_molt_time_monotonic_ns = cast(
    Callable[[], int], _load_intrinsic("_molt_time_monotonic_ns")
)
_molt_time_time = cast(Callable[[], float], _load_intrinsic("_molt_time_time"))
_molt_time_time_ns = cast(Callable[[], int], _load_intrinsic("_molt_time_time_ns"))
_molt_getpid = cast(Callable[[], int], _load_intrinsic("_molt_getpid"))
_molt_getcwd = cast(Callable[[], str], _load_intrinsic("_molt_getcwd"))
_molt_env_get_raw = cast(Callable[..., object], _load_intrinsic("_molt_env_get_raw"))
_molt_env_snapshot = cast(Callable[[], object], _load_intrinsic("_molt_env_snapshot"))
_molt_errno_constants = cast(
    Callable[[], tuple[dict[str, int], dict[int, str]]],
    _load_intrinsic("_molt_errno_constants"),
)
_molt_path_exists = cast(
    Callable[[object], bool],
    _load_intrinsic("_molt_path_exists"),
)
_molt_path_listdir = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_path_listdir"),
)
_molt_path_mkdir = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_path_mkdir"),
)
_molt_path_unlink = cast(
    Callable[[object], None],
    _load_intrinsic("_molt_path_unlink"),
)
_molt_path_rmdir = cast(
    Callable[[object], None],
    _load_intrinsic("_molt_path_rmdir"),
)
_molt_path_chmod = cast(
    Callable[[object, object], None],
    _load_intrinsic("_molt_path_chmod"),
)
_molt_os_close = cast(Callable[[object], object], _load_intrinsic("_molt_os_close"))
_molt_os_dup = cast(Callable[[object], object], _load_intrinsic("_molt_os_dup"))
_molt_os_get_inheritable = cast(
    Callable[[object], object],
    _load_intrinsic("_molt_os_get_inheritable"),
)
_molt_os_set_inheritable = cast(
    Callable[[object, object], object],
    _load_intrinsic("_molt_os_set_inheritable"),
)
_molt_struct_pack = cast(
    Callable[[object, object], object], _load_intrinsic("_molt_struct_pack")
)
_molt_struct_unpack = cast(
    Callable[[object, object], object], _load_intrinsic("_molt_struct_unpack")
)
_molt_struct_calcsize = cast(
    Callable[[object], object], _load_intrinsic("_molt_struct_calcsize")
)
