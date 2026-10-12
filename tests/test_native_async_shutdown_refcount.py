from __future__ import annotations

import ast
from pathlib import Path

import pytest


# Compiled shutdown acceptance lives in the strict asyncio_task_basic fixture
# selected by runtime.public-consumers.native.
# The native drop call is the boundary under test; no runtime is loaded by these
# controls. Compile the actual finalizer and its defaults, then retire its module
# namespace exactly as module shutdown does. The table describes constructor
# handle ownership, independently of the finalizer implementation.
_HANDLE_FINALIZERS = [
    ("_sqlite3.py", "Cursor", "_handle", "_MOLT_SQLITE3_CURSOR_DROP"),
    ("_sqlite3.py", "Connection", "_handle", "_MOLT_SQLITE3_CLOSE"),
    ("ssl.py", "SSLSocket", "_handle", "_MOLT_SSL_SOCKET_DROP"),
    ("queue.py", "Queue", "_handle", "_MOLT_QUEUE_DROP"),
    ("_queue.py", "SimpleQueue", "_handle", "_MOLT_QUEUE_DROP"),
    ("lzma.py", "LZMACompressor", "_handle", "_MOLT_LZMA_COMPRESSOR_DROP"),
    ("lzma.py", "LZMADecompressor", "_handle", "_MOLT_LZMA_DECOMPRESSOR_DROP"),
    ("ipaddress.py", "IPv4Address", "_handle", "_molt_ipaddress_drop"),
    ("ipaddress.py", "IPv6Address", "_handle", "_molt_ipaddress_v6_drop"),
    ("ipaddress.py", "IPv4Network", "_handle", "_molt_ipaddress_v4_network_drop"),
    ("bz2.py", "BZ2Compressor", "_handle", "_MOLT_BZ2_COMPRESSOR_DROP"),
    ("bz2.py", "BZ2Decompressor", "_handle", "_MOLT_BZ2_DECOMPRESSOR_DROP"),
    ("functools.py", "singledispatch", "_handle", "_MOLT_SD_DROP"),
    ("hashlib.py", "_Hash", "_handle", "_molt_hash_drop"),
    ("codecs.py", "IncrementalEncoder", "_handle", "_molt_inc_enc_drop"),
    ("codecs.py", "IncrementalDecoder", "_handle", "_molt_inc_dec_drop"),
    ("codecs.py", "StreamReader", "_sr_handle", "_molt_stream_reader_drop"),
    ("decimal.py", "Context", "_handle", "_MOLT_DECIMAL_CONTEXT_DROP"),
    ("decimal.py", "Decimal", "_handle", "_MOLT_DECIMAL_DROP"),
    ("zlib.py", "Compress", "_handle", "_MOLT_ZLIB_COMPRESSOBJ_DROP"),
    ("zlib.py", "Decompress", "_handle", "_MOLT_ZLIB_DECOMPRESSOBJ_DROP"),
    ("_collections.py", "OrderedDict", "_handle", "_MOLT_ORDEREDDICT_DROP"),
    ("configparser.py", "RawConfigParser", "_handle", "_molt_configparser_drop"),
    ("csv.py", "_Reader", "_handle", "_MOLT_CSV_READER_DROP"),
    ("csv.py", "_Writer", "_handle", "_MOLT_CSV_WRITER_DROP"),
    ("fractions.py", "Fraction", "_handle", "_MOLT_FRACTION_DROP"),
    (
        "contextlib.py",
        "AsyncExitStack",
        "_molt_state",
        "_MOLT_CONTEXTLIB_EXITSTACK_DROP",
    ),
    ("contextlib.py", "ExitStack", "_molt_state", "_MOLT_CONTEXTLIB_EXITSTACK_DROP"),
    ("graphlib.py", "TopologicalSorter", "_handle", "_MOLT_GRAPHLIB_DROP"),
    ("hmac.py", "HMAC", "_handle", "_molt_hmac_drop"),
    ("ssl.py", "SSLContext", "_handle", "_MOLT_SSL_CONTEXT_DROP"),
    ("_multiprocessing.py", "SemLock", "_handle", "_molt_semlock_drop"),
    ("zoneinfo/__init__.py", "ZoneInfo", "_handle", "_molt_zoneinfo_drop"),
    ("urllib/response.py", "addinfourl", "_molt_handle", "_MOLT_RESP_DROP"),
    ("html/parser.py", "HTMLParser", "_handle", "_molt_html_parser_drop"),
    ("email/message.py", "Message", "_handle", "_MOLT_EMAIL_MESSAGE_DROP"),
    ("http/client.py", "HTTPMessage", "_handle", "_MOLT_HTTP_MESSAGE_DROP"),
    ("http/client.py", "HTTPResponse", "_handle", "_MOLT_HTTP_RESP_DROP"),
    ("http/client.py", "HTTPConnection", "_conn_handle", "_MOLT_HTTP_CONN_DROP"),
    ("collections/__init__.py", "deque", "_handle", "_MOLT_DEQUE_DROP"),
    ("collections/__init__.py", "defaultdict", "_dd_handle", "_MOLT_DEFAULTDICT_DROP"),
    ("logging/__init__.py", "_RLock", "_handle", "_RLOCK_DROP"),
    ("logging/__init__.py", "LogRecord", "_handle", "_RECORD_DROP"),
    ("logging/__init__.py", "Formatter", "_handle", "_FORMATTER_DROP"),
    ("logging/__init__.py", "Handler", "_handle", "_HANDLER_DROP"),
    ("logging/__init__.py", "Logger", "_handle", "_LOGGER_DROP"),
    ("threading.py", "Condition", "_handle", "_condition_drop"),
    ("threading.py", "Event", "_handle", "_event_drop"),
    ("threading.py", "Semaphore", "_handle", "_semaphore_drop"),
    ("threading.py", "Barrier", "_handle", "_barrier_drop"),
    ("threading.py", "local", "_handle", "_local_drop"),
    ("threading.py", "Thread", "_handle", "_thread_drop"),
    ("_thread.py", "LockType", "_handle", "_lock_drop"),
    ("_thread.py", "RLock", "_handle", "_rlock_drop"),
    ("asyncio/events.py", "_EventLoop", "_loop_handle", "_molt_event_loop_drop"),
    (
        "asyncio/events.py",
        "_ReadPipeTransport",
        "_pipe_handle",
        "_molt_pipe_transport_drop",
    ),
    (
        "asyncio/events.py",
        "_WritePipeTransport",
        "_pipe_handle",
        "_molt_pipe_transport_drop",
    ),
    ("asyncio/futures.py", "Future", "_fut_handle", "_molt_asyncio_future_drop"),
    ("asyncio/tasks.py", "CancellationToken", "_token", "_molt_cancel_token_drop"),
    ("asyncio/locks.py", "Event", "_evt_handle", "_molt_asyncio_event_drop"),
    ("asyncio/locks.py", "Lock", "_lock_handle", "_molt_asyncio_lock_drop"),
    ("asyncio/locks.py", "Semaphore", "_sem_handle", "_molt_asyncio_semaphore_drop"),
    ("asyncio/queues.py", "Queue", "_q_handle", "_molt_asyncio_queue_drop"),
    ("asyncio/streams.py", "StreamReader", "_reader", "_molt_socket_reader_drop"),
    (
        "asyncio/streams.py",
        "ProcessStreamReader",
        "_reader",
        "_molt_stream_reader_drop",
    ),
    ("asyncio/subprocess.py", "Process", "_handle", "_molt_process_drop"),
    ("concurrent/futures/__init__.py", "Future", "_handle", "_MOLT_FUTURE_DROP"),
    (
        "concurrent/futures/__init__.py",
        "ThreadPoolExecutor",
        "_handle",
        "_MOLT_THREADPOOL_DROP",
    ),
]


# Preserve the existing __del__ error policy while making retirement exact-once.
_SUPPRESSED_FINALIZERS = {
    ("_sqlite3.py", "Cursor"),
    ("_sqlite3.py", "Connection"),
    ("ssl.py", "SSLSocket"),
    ("queue.py", "Queue"),
    ("_queue.py", "SimpleQueue"),
    ("lzma.py", "LZMACompressor"),
    ("lzma.py", "LZMADecompressor"),
    ("ipaddress.py", "IPv4Address"),
    ("ipaddress.py", "IPv6Address"),
    ("ipaddress.py", "IPv4Network"),
    ("bz2.py", "BZ2Compressor"),
    ("bz2.py", "BZ2Decompressor"),
    ("functools.py", "singledispatch"),
    ("hashlib.py", "_Hash"),
    ("codecs.py", "IncrementalEncoder"),
    ("codecs.py", "IncrementalDecoder"),
    ("codecs.py", "StreamReader"),
    ("decimal.py", "Context"),
    ("decimal.py", "Decimal"),
    ("zlib.py", "Compress"),
    ("zlib.py", "Decompress"),
    ("_collections.py", "OrderedDict"),
    ("configparser.py", "RawConfigParser"),
    ("csv.py", "_Reader"),
    ("csv.py", "_Writer"),
    ("fractions.py", "Fraction"),
    ("contextlib.py", "AsyncExitStack"),
    ("contextlib.py", "ExitStack"),
    ("graphlib.py", "TopologicalSorter"),
    ("hmac.py", "HMAC"),
    ("ssl.py", "SSLContext"),
    ("_multiprocessing.py", "SemLock"),
    ("zoneinfo/__init__.py", "ZoneInfo"),
    ("urllib/response.py", "addinfourl"),
    ("html/parser.py", "HTMLParser"),
    ("http/client.py", "HTTPMessage"),
    ("http/client.py", "HTTPResponse"),
    ("http/client.py", "HTTPConnection"),
    ("collections/__init__.py", "deque"),
    ("collections/__init__.py", "defaultdict"),
    ("logging/__init__.py", "_RLock"),
    ("logging/__init__.py", "LogRecord"),
    ("logging/__init__.py", "Formatter"),
    ("logging/__init__.py", "Handler"),
    ("logging/__init__.py", "Logger"),
    ("threading.py", "local"),
}
_MIGRATED_FINALIZERS = {
    ("_sqlite3.py", "Cursor"),
    ("_sqlite3.py", "Connection"),
    ("ssl.py", "SSLSocket"),
    ("queue.py", "Queue"),
    ("_queue.py", "SimpleQueue"),
    ("lzma.py", "LZMACompressor"),
    ("lzma.py", "LZMADecompressor"),
    ("ipaddress.py", "IPv4Address"),
    ("ipaddress.py", "IPv6Address"),
    ("ipaddress.py", "IPv4Network"),
    ("bz2.py", "BZ2Compressor"),
    ("bz2.py", "BZ2Decompressor"),
    ("functools.py", "singledispatch"),
    ("hashlib.py", "_Hash"),
    ("codecs.py", "IncrementalEncoder"),
    ("codecs.py", "IncrementalDecoder"),
    ("codecs.py", "StreamReader"),
    ("decimal.py", "Context"),
    ("decimal.py", "Decimal"),
    ("zlib.py", "Compress"),
    ("zlib.py", "Decompress"),
    ("_collections.py", "OrderedDict"),
    ("configparser.py", "RawConfigParser"),
    ("csv.py", "_Reader"),
    ("csv.py", "_Writer"),
    ("fractions.py", "Fraction"),
    ("contextlib.py", "AsyncExitStack"),
    ("contextlib.py", "ExitStack"),
    ("graphlib.py", "TopologicalSorter"),
    ("hmac.py", "HMAC"),
    ("ssl.py", "SSLContext"),
    ("_multiprocessing.py", "SemLock"),
    ("zoneinfo/__init__.py", "ZoneInfo"),
    ("urllib/response.py", "addinfourl"),
    ("html/parser.py", "HTMLParser"),
    ("email/message.py", "Message"),
    ("http/client.py", "HTTPMessage"),
    ("http/client.py", "HTTPResponse"),
    ("http/client.py", "HTTPConnection"),
    ("collections/__init__.py", "deque"),
    ("collections/__init__.py", "defaultdict"),
    ("logging/__init__.py", "_RLock"),
    ("logging/__init__.py", "LogRecord"),
    ("logging/__init__.py", "Formatter"),
    ("logging/__init__.py", "Handler"),
    ("logging/__init__.py", "Logger"),
    ("threading.py", "Condition"),
    ("threading.py", "Event"),
    ("threading.py", "Semaphore"),
    ("threading.py", "Barrier"),
    ("threading.py", "local"),
    ("threading.py", "Thread"),
    ("_thread.py", "LockType"),
    ("_thread.py", "RLock"),
}


def _handle_finalizer_class(relative_path, class_name, drop_name, drop, forget=None):
    path = Path(__file__).resolve().parents[1] / "src/molt/stdlib" / relative_path
    tree = ast.parse(path.read_text(encoding="utf-8"))
    owner = next(
        node
        for node in tree.body
        if isinstance(node, ast.ClassDef) and node.name == class_name
    )
    finalizer = next(
        node
        for node in owner.body
        if isinstance(node, ast.FunctionDef) and node.name == "__del__"
    )
    unit = ast.parse("from __future__ import annotations\nclass FinalizerOwner: pass\n")
    wrapper = unit.body[1]
    assert isinstance(wrapper, ast.ClassDef)
    wrapper.body = [
        node
        for node in owner.body
        if isinstance(node, ast.FunctionDef) and node.name in {"_drop", "__del__"}
    ]
    assert finalizer in wrapper.body
    namespace = {
        drop_name: drop,
        "_PROCESS_WAIT_FUTURES": {},
        "_thread_registry_forget": forget
        if forget is not None
        else lambda handle: None,
    }
    exec(compile(ast.fix_missing_locations(unit), str(path), "exec"), namespace)
    return namespace["FinalizerOwner"], namespace


def test_email_message_import_then_retired_globals_releases_handle_once():
    import importlib.util

    from tests.stdlib_intrinsic_registry import intrinsic_registry

    calls = []
    message = None

    def drop(handle):
        calls.append(handle)
        message.__del__()

    # Load the entire real module in source order: extracting only __del__ and
    # prebinding its defaults cannot detect a definition-time missing intrinsic.
    path = Path(__file__).resolve().parents[1] / "src/molt/stdlib/email/message.py"
    with intrinsic_registry(
        {
            "molt_email_message_new": lambda: 0,
            "molt_email_message_set": lambda *_: None,
            "molt_email_message_set_content": lambda *_: None,
            "molt_email_message_add_alternative": lambda *_: None,
            "molt_email_message_add_attachment": lambda *_: None,
            "molt_email_message_drop": drop,
        }
    ):
        spec = importlib.util.spec_from_file_location("molt_test_email_message", path)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        owner = module.Message
        message = owner()
        assert message._handle == 0
        assert calls == []
        module.__dict__.clear()
        message.__del__()
        message.__del__()
        assert calls == [0]
        owner.__new__(owner).__del__()  # construction failed before handle publication
        assert calls == [0]


@pytest.mark.parametrize("relative_path,class_name,field,drop_name", _HANDLE_FINALIZERS)
def test_handle_finalizer_survives_retired_globals_exactly_once(
    relative_path, class_name, field, drop_name
):
    calls = []
    instance = None

    def drop(handle):
        calls.append(handle)
        # A native owner's retirement can release Python payloads and reenter.
        # The handle must have been detached before that callback.
        instance.__del__()

    owner, namespace = _handle_finalizer_class(
        relative_path, class_name, drop_name, drop
    )
    instance = owner()
    setattr(instance, field, 73)
    if class_name == "CancellationToken":
        instance._owned = True
    if class_name == "ThreadPoolExecutor":
        instance._shutdown = False
    wait_futures = namespace["_PROCESS_WAIT_FUTURES"]
    wait_futures[id(instance)] = object()
    namespace.clear()
    instance.__del__()
    instance.__del__()
    assert calls == [73]
    if class_name == "Process":
        assert id(instance) not in wait_futures
    owner().__del__()  # constructor failed before publishing a handle
    assert calls == [73]


@pytest.mark.parametrize(
    "relative_path,class_name,field,drop_name",
    [
        row
        for row in _HANDLE_FINALIZERS
        if row[1] in {"CancellationToken", "ThreadPoolExecutor"}
    ],
)
def test_handle_finalizer_does_not_drop_borrowed_or_shutdown_owner(
    relative_path, class_name, field, drop_name
):
    calls = []
    owner, namespace = _handle_finalizer_class(
        relative_path, class_name, drop_name, calls.append
    )
    instance = owner()
    setattr(instance, field, 73)
    if class_name == "CancellationToken":
        instance._owned = False
    else:
        instance._shutdown = True
    namespace.clear()
    instance.__del__()
    assert calls == []


def test_handle_finalizer_process_detaches_before_wait_future_retirement():
    calls = []
    row = next(row for row in _HANDLE_FINALIZERS if row[1] == "Process")
    owner, namespace = _handle_finalizer_class(row[0], row[1], row[3], calls.append)
    instance = owner()
    instance._handle = 73

    class WaitFuture:
        def __del__(self):
            instance.__del__()

    wait_futures = namespace["_PROCESS_WAIT_FUTURES"]
    wait_futures[id(instance)] = WaitFuture()
    namespace.clear()
    instance.__del__()
    assert calls == [73]
    assert wait_futures == {}


@pytest.mark.parametrize(
    "relative_path,class_name,field,drop_name",
    [
        row
        for row in _HANDLE_FINALIZERS
        if row[:2] in _MIGRATED_FINALIZERS or row[1] == "Queue"
    ],
)
def test_handle_finalizer_preserves_drop_failure_and_prevents_duplicate_retirement(
    relative_path, class_name, field, drop_name
):
    error = RuntimeError("native drop witness")
    calls = []

    def drop(handle):
        calls.append(handle)
        raise error

    owner, namespace = _handle_finalizer_class(
        relative_path, class_name, drop_name, drop
    )
    instance = owner()
    setattr(instance, field, 73)
    namespace.clear()
    if (relative_path, class_name) in _SUPPRESSED_FINALIZERS:
        instance.__del__()
    else:
        with pytest.raises(RuntimeError) as raised:
            instance.__del__()
        assert raised.value is error
    instance.__del__()
    assert calls == [73]


@pytest.mark.parametrize("class_name", ["_ReadPipeTransport", "_WritePipeTransport"])
def test_handle_finalizer_preserves_missing_pipe_intrinsic_diagnostic(class_name):
    owner, namespace = _handle_finalizer_class(
        "asyncio/events.py", class_name, "_molt_pipe_transport_drop", None
    )
    namespace.clear()
    instance = owner()
    instance.__del__()
    instance._pipe_handle = 73
    with pytest.raises(
        RuntimeError, match="^asyncio intrinsic not available: pipe_transport_drop$"
    ):
        instance.__del__()
    instance.__del__()


def test_thread_finalizer_detaches_before_registry_forget_and_still_drops_on_error():
    for fail in (False, True):
        calls = []
        failure = RuntimeError("thread registry failure")
        instance = None

        def forget(handle):
            assert instance._handle is None
            assert instance._synthetic_alive is False
            calls.append(("forget", handle))
            instance.__del__()
            if fail:
                raise failure

        def drop(handle):
            calls.append(("drop", handle))
            instance.__del__()

        owner, namespace = _handle_finalizer_class(
            "threading.py", "Thread", "_thread_drop", drop, forget
        )
        instance = owner()
        instance._handle = 73
        instance._synthetic_alive = True
        namespace.clear()
        if fail:
            with pytest.raises(RuntimeError) as raised:
                instance.__del__()
            assert raised.value is failure
        else:
            instance.__del__()
        instance.__del__()
        assert calls == [("forget", 73), ("drop", 73)]


@pytest.mark.parametrize(
    "relative_path,class_name,field,drop_name",
    [row for row in _HANDLE_FINALIZERS if row[:2] in _MIGRATED_FINALIZERS],
)
def test_handle_finalizer_detach_bypasses_subclass_setter(
    relative_path, class_name, field, drop_name
):
    calls = []
    owner, namespace = _handle_finalizer_class(
        relative_path, class_name, drop_name, calls.append
    )

    class RejectWrites(owner):
        def __setattr__(self, name, value):
            raise AssertionError("cleanup routed through custom setter")

    instance = RejectWrites()
    object.__setattr__(instance, field, 0)
    namespace.clear()
    instance.__del__()
    instance.__del__()
    assert calls == [0]  # None, not falsiness, is the detached sentinel.
    assert object.__getattribute__(instance, field) is None


def _cleanup_owner(
    relative_path, class_name, bindings, *, include_close=True, additional_methods=()
):
    """Load real cleanup methods, including their real base cleanup protocol."""
    path = Path(__file__).resolve().parents[1] / "src/molt/stdlib" / relative_path
    tree = ast.parse(path.read_text())
    source = next(
        n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == class_name
    )
    methods = {"__del__", "_release_handle", *additional_methods}
    if include_close:
        methods.add("close")
    unit = ast.parse("from __future__ import annotations\nclass Owner: pass\n")
    owner = unit.body[1]
    owner.body = [
        n for n in source.body if isinstance(n, ast.FunctionDef) and n.name in methods
    ]
    if class_name == "_IntrinsicSelectorImpl":
        base_source = next(
            n
            for n in tree.body
            if isinstance(n, ast.ClassDef) and n.name == "_BaseSelectorImpl"
        )
        base = ast.parse("class Base: pass").body[0]
        base.body = [
            n
            for n in base_source.body
            if isinstance(n, ast.FunctionDef) and n.name == "close"
        ]
        unit.body.insert(1, base)
        owner.bases = [ast.Name(id="Base", ctx=ast.Load())]
    for name in ("_require_socket_intrinsic", "_compress_payload"):
        unit.body[1:1] = [
            n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == name
        ]
    namespace = dict(bindings)
    exec(compile(ast.fix_missing_locations(unit), str(path), "exec"), namespace)
    return namespace["Owner"], namespace


_CLOSE_DROP_OWNERS = [
    (
        "select.py",
        "_SelectorBackend",
        "_handle",
        "_MOLT_SELECTOR_CLOSE",
        "_MOLT_SELECTOR_DROP",
    ),
    (
        "select.py",
        "_PollObject",
        "_handle",
        "_MOLT_SELECTOR_CLOSE",
        "_MOLT_SELECTOR_DROP",
    ),
    (
        "selectors.py",
        "_IntrinsicSelectorImpl",
        "_selector_handle",
        "_MOLT_SELECTOR_CLOSE",
        "_MOLT_SELECTOR_DROP",
    ),
    ("socket.py", "socket", "_handle", "_molt_socket_close", "_molt_socket_drop"),
    ("gzip.py", "GzipFile", "_handle", "_MOLT_GZIP_CLOSE", "_MOLT_GZIP_DROP"),
    ("bz2.py", "BZ2File", "_handle", "_MOLT_BZ2_FILE_CLOSE", "_MOLT_BZ2_FILE_DROP"),
    ("lzma.py", "LZMAFile", "_handle", "_MOLT_LZMA_FILE_CLOSE", "_MOLT_LZMA_FILE_DROP"),
    ("tarfile.py", "TarFile", "_handle", "_molt_tarfile_close", "_molt_tarfile_drop"),
]


@pytest.mark.parametrize(
    "relative_path,class_name,field,close_name,drop_name", _CLOSE_DROP_OWNERS
)
@pytest.mark.parametrize("failure_stage", [None, "close", "drop"])
def test_compound_cleanup_retires_owner_before_callbacks_and_preserves_errors(
    relative_path, class_name, field, close_name, drop_name, failure_stage
):
    calls = []
    error = OSError("finish or release witness")
    instance = None

    def callback(stage, handle):
        assert getattr(instance, field) is None
        calls.append((stage, handle))
        instance.__del__()
        if stage == failure_stage:
            raise error

    owner, namespace = _cleanup_owner(
        relative_path,
        class_name,
        {
            close_name: lambda h: callback("close", h),
            drop_name: lambda h: callback("drop", h),
            "_MOLT_GZIP_COMPRESS": lambda *args: b"",
            "BadGzipFile": OSError,
        },
    )
    instance = owner()
    object.__setattr__(instance, field, 0)
    if class_name == "_IntrinsicSelectorImpl":
        instance._fd_to_key = {7: object()}
        instance._map = object()
    namespace.clear()
    if failure_stage:
        with pytest.raises(OSError) as raised:
            instance._release_handle()
        assert raised.value is error
    else:
        instance._release_handle()
    instance.__del__()
    instance.__del__()
    owner().__del__()  # a constructor which never published a native owner
    assert calls == [("close", 0), ("drop", 0)]
    if class_name == "_IntrinsicSelectorImpl":
        assert instance._fd_to_key == {}
        assert instance._map is None


@pytest.mark.parametrize(
    "relative_path,class_name,field,close_name,drop_name", _CLOSE_DROP_OWNERS
)
def test_compound_finalizer_preserves_suppression_after_namespace_retirement(
    relative_path, class_name, field, close_name, drop_name
):
    calls = []

    def fail(handle):
        calls.append(("close", handle))
        raise OSError("close witness")

    owner, namespace = _cleanup_owner(
        relative_path,
        class_name,
        {
            close_name: fail,
            drop_name: lambda h: calls.append(("drop", h)),
            "_MOLT_GZIP_COMPRESS": lambda *args: b"",
            "BadGzipFile": OSError,
        },
    )
    instance = owner()
    setattr(instance, field, 73)
    namespace.clear()
    instance.__del__()
    instance.__del__()
    assert calls == [("close", 73), ("drop", 73)]


def test_sqlite_cursor_closed_owner_still_retires_registry_once():
    for already_closed in (False, True):
        calls = []
        owner, namespace = _cleanup_owner(
            "_sqlite3.py",
            "Cursor",
            {
                "_MOLT_SQLITE3_CURSOR_DROP": lambda h: calls.append(("drop", h)),
                "_MOLT_SQLITE3_CURSOR_CLOSE": lambda h: calls.append(("close", h)),
                "_call_intrinsic": lambda fn, *args: fn(*args),
            },
        )
        instance = owner()
        instance._handle = 73
        instance._closed = False
        if already_closed:
            instance.close()
        namespace.clear()
        instance.__del__()
        instance.__del__()
        owner().__del__()
        assert calls == ([("close", 73)] if already_closed else []) + [("drop", 73)]


@pytest.mark.parametrize(
    "relative_path,class_name,drop_name",
    [
        ("_sqlite3.py", "Connection", "_MOLT_SQLITE3_CLOSE"),
        ("ssl.py", "SSLSocket", "_MOLT_SSL_SOCKET_DROP"),
    ],
)
@pytest.mark.parametrize("closed", [False, True])
def test_closed_connection_has_already_transferred_native_owner(
    relative_path, class_name, drop_name, closed
):
    calls = []
    owner, namespace = _cleanup_owner(
        relative_path, class_name, {drop_name: calls.append}, include_close=False
    )
    instance = owner()
    instance._handle = 0
    instance._closed = closed
    namespace.clear()
    instance.__del__()
    instance.__del__()
    owner().__del__()
    assert calls == ([] if closed else [0])
    assert instance._handle is None


@pytest.mark.parametrize("failure_index", range(7))
def test_popen_detaches_all_native_owners_before_any_drop_and_continues_cleanup(
    failure_index,
):
    fields = (
        "_stdin_stream",
        "_stdout_stream",
        "_stderr_stream",
        "_stdout_reader",
        "_stderr_reader",
        "_handle",
    )
    calls = []
    instance = None

    def drop(handle):
        assert all(getattr(instance, field) is None for field in fields)
        calls.append(handle)
        instance.__del__()
        if handle == failure_index:
            raise OSError("one native owner failed")

    owner, namespace = _cleanup_owner(
        "subprocess.py",
        "Popen",
        {
            "_MOLT_STREAM_DROP": drop,
            "_MOLT_STREAM_READER_DROP": drop,
            "_MOLT_PROCESS_DROP": drop,
        },
        include_close=False,
    )
    instance = owner()
    for handle, field in enumerate(fields):
        setattr(instance, field, handle)
    namespace.clear()
    instance.__del__()
    instance.__del__()
    owner().__del__()
    assert calls == list(range(6))


@pytest.mark.parametrize("failure", [False, True])
def test_dbm_close_retires_owner_once_and_preserves_unsuppressed_error(failure):
    calls = []
    error = OSError("database sync witness")
    instance = None

    def close(handle):
        assert instance._handle is None
        assert instance._closed
        calls.append(handle)
        instance.__del__()
        if failure:
            raise error

    owner, namespace = _cleanup_owner(
        "dbm/dumb.py", "_Database", {"_MOLT_DBM_DUMB_CLOSE": close}
    )
    instance = owner()
    instance._handle = 0
    namespace.clear()
    if failure:
        with pytest.raises(OSError) as raised:
            instance.__del__()
        assert raised.value is error
    else:
        instance.__del__()
    instance.__del__()
    owner().__del__()
    assert calls == [0]


@pytest.mark.parametrize("failure_stage", [None, "compress", "write"])
def test_gzip_borrowed_file_close_finishes_once_without_closing_borrowed_file(
    failure_stage,
):
    calls = []
    error = OSError("finish witness")
    instance = None

    class BadGzipFile(OSError):
        pass

    def compress(data, level, mtime):
        assert instance._closed
        calls.append(("compress", data, level, mtime))
        instance.__del__()
        if failure_stage == "compress":
            raise error
        return b"compressed"

    class BorrowedFile:
        def write(self, data):
            assert instance._closed
            calls.append(("write", data))
            instance.__del__()
            if failure_stage == "write":
                raise error

        def close(self):
            pytest.fail("GzipFile must not close a borrowed file")

    owner, namespace = _cleanup_owner(
        "gzip.py",
        "GzipFile",
        {
            "_MOLT_GZIP_CLOSE": lambda h: pytest.fail("no native file owner"),
            "_MOLT_GZIP_DROP": lambda h: pytest.fail("no native file owner"),
            "_MOLT_GZIP_COMPRESS": compress,
            "BadGzipFile": BadGzipFile,
        },
    )
    instance = owner()
    instance._handle = None
    instance._fileobj = BorrowedFile()
    instance._write_buf = bytearray(b"payload")
    instance._compresslevel = 7
    instance._mtime = None
    namespace.clear()
    if failure_stage:
        with pytest.raises(OSError) as raised:
            instance.close()
        if failure_stage == "compress":
            assert isinstance(raised.value, BadGzipFile)
            assert raised.value.__cause__ is error
        else:
            assert raised.value is error
    else:
        instance.close()
    instance.__del__()
    assert calls == [("compress", b"payload", 7, 0)] + (
        [] if failure_stage == "compress" else [("write", b"compressed")]
    )


@pytest.mark.parametrize("fail_shutdown", [False, True])
def test_ssl_unwrap_transfers_live_transport_only_after_success(fail_shutdown):
    import errno
    import os
    import socket

    transport, peer = socket.socketpair()
    owned = {}
    calls = []
    error = OSError("TLS shutdown witness")

    def drop(handle):
        calls.append(("drop", handle))
        os.close(owned.pop(handle))

    def unwrap(handle):
        calls.append(("unwrap", handle))
        if fail_shutdown:
            raise error
        os.close(owned.pop(handle))
        return None  # terminal consume, not an owned descriptor return

    owner, namespace = _cleanup_owner(
        "ssl.py",
        "SSLSocket",
        {
            "_MOLT_SSL_SOCKET_UNWRAP": unwrap,
            "_MOLT_SSL_SOCKET_CLOSE": drop,
            "_MOLT_SSL_SOCKET_DROP": drop,
        },
        additional_methods=("__init__", "unwrap"),
    )
    try:
        descriptor = os.dup(transport.fileno())
        owned[73] = descriptor
        instance = owner(73, sock=transport)
        if fail_shutdown:
            with pytest.raises(OSError) as raised:
                instance.unwrap()
            assert raised.value is error
            assert instance._handle == 73 and instance._closed is False
            os.fstat(descriptor)  # failed shutdown did not transfer ownership
            namespace.clear()
            instance.__del__()
            assert calls == [("unwrap", 73), ("drop", 73)]
        else:
            assert instance.unwrap() is transport
            assert instance._handle is None and instance._closed
            with pytest.raises(ValueError):
                instance.unwrap()
            namespace.clear()
            instance.__del__()
            assert calls == [("unwrap", 73)]
        instance.__del__()
        owner.__new__(owner).__del__()  # constructor failed before handle publication
        with pytest.raises(OSError) as raised:
            os.fstat(descriptor)
        assert raised.value.errno == errno.EBADF
        transport.sendall(b"raw")
        assert peer.recv(3) == b"raw"
        # A successor wrapper may acquire its own duplicate of the same live socket.
        successor_fd = os.dup(transport.fileno())
        owned[74] = successor_fd
        successor = owner(74, sock=transport)
        successor.__del__()
        successor.__del__()
        assert calls[-1] == ("drop", 74)
        assert owned == {}
        transport.sendall(b"next")
        assert peer.recv(4) == b"next"
    finally:
        for descriptor in owned.values():
            os.close(descriptor)
        transport.close()
        peer.close()


# Prior source placed these attribute reads inside its suppression boundary.
_SUPPRESSED_ATTRIBUTE_READS = {
    ("decimal.py", "Context"),
    ("hashlib.py", "_Hash"),
    ("queue.py", "Queue"),
    ("ssl.py", "SSLSocket"),
    ("logging/__init__.py", "LogRecord"),
    ("ssl.py", "SSLContext"),
    ("ipaddress.py", "IPv6Address"),
    ("hmac.py", "HMAC"),
    ("logging/__init__.py", "Formatter"),
    ("ipaddress.py", "IPv4Network"),
    ("_queue.py", "SimpleQueue"),
    ("ipaddress.py", "IPv4Address"),
    ("decimal.py", "Decimal"),
    ("contextlib.py", "ExitStack"),
    ("logging/__init__.py", "_RLock"),
    ("logging/__init__.py", "Logger"),
    ("logging/__init__.py", "Handler"),
    ("contextlib.py", "AsyncExitStack"),
}


@pytest.mark.parametrize(
    "relative_path,class_name,field,drop_name",
    [
        row
        for row in _HANDLE_FINALIZERS
        if row[:2] in _SUPPRESSED_ATTRIBUTE_READS
        or row[:2] == ("_collections.py", "OrderedDict")
    ],
)
def test_finalizer_preserves_attribute_read_failure_policy(
    relative_path, class_name, field, drop_name
):
    calls = []
    error = RuntimeError("attribute read witness")
    owner, namespace = _handle_finalizer_class(
        relative_path, class_name, drop_name, calls.append
    )

    class RejectReads(owner):
        reject = True

        def __getattribute__(self, name):
            if name == field and object.__getattribute__(self, "reject"):
                raise error
            return object.__getattribute__(self, name)

    instance = RejectReads()
    object.__setattr__(instance, field, 73)
    namespace.clear()
    if (relative_path, class_name) in _SUPPRESSED_ATTRIBUTE_READS:
        instance.__del__()
    else:
        with pytest.raises(RuntimeError) as raised:
            instance.__del__()
        assert raised.value is error
    assert calls == []
    instance.reject = False
    instance.__del__()
    assert calls == [73]


def test_native_cleanup_family_has_behavioral_coverage_and_no_live_global_intrinsic_reads():
    root = Path(__file__).resolve().parents[1] / "src/molt/stdlib"
    rows = []
    violations = []
    for p in root.rglob("*.py"):
        tree = ast.parse(p.read_text())
        nodes = list(ast.walk(tree))
        bindings = set()
        aliases = {}
        for node in nodes:
            if isinstance(node, ast.Assign):
                targets = [
                    target.id for target in node.targets if isinstance(target, ast.Name)
                ]
                value = node.value
                if (
                    isinstance(value, ast.Call)
                    and value.args
                    and isinstance(value.args[0], ast.Constant)
                    and isinstance(value.args[0].value, str)
                    and value.args[0].value.startswith("molt_")
                ):
                    bindings.update(targets)
                elif isinstance(value, ast.Name):
                    aliases.setdefault(value.id, []).extend(targets)
            elif isinstance(node, ast.ImportFrom):
                bindings.update(
                    name.asname or name.name
                    for name in node.names
                    if name.name.startswith(("_molt_", "_MOLT_"))
                )
        pending = list(bindings)
        while pending:
            source = pending.pop()
            for target in aliases.get(source, ()):
                if target not in bindings:
                    bindings.add(target)
                    pending.append(target)
        for cl in nodes:
            if not isinstance(cl, ast.ClassDef):
                continue
            methods = {n.name: n for n in cl.body if isinstance(n, ast.FunctionDef)}
            if "__del__" not in methods:
                continue
            todo = ["__del__"]
            seen = set()
            native = False
            while todo:
                name = todo.pop()
                if name in seen:
                    continue
                seen.add(name)
                f = methods[name]
                native |= any(
                    isinstance(n, ast.Name) and n.id in bindings for n in ast.walk(f)
                )
                for stmt in f.body:
                    for n in ast.walk(stmt):
                        if (
                            isinstance(n, ast.Name)
                            and isinstance(n.ctx, ast.Load)
                            and n.id in bindings
                        ):
                            violations.append(
                                (
                                    str(p.relative_to(root)),
                                    cl.name,
                                    name,
                                    n.id,
                                    n.lineno,
                                )
                            )
                        if (
                            isinstance(n, ast.Call)
                            and isinstance(n.func, ast.Attribute)
                            and isinstance(n.func.value, ast.Name)
                            and n.func.value.id == "self"
                            and n.func.attr in methods
                        ):
                            todo.append(n.func.attr)
            if native:
                rows.append((str(p.relative_to(root)), cl.name))
    # Font deletes a named Tcl resource in a shared borrowed app, not an
    # opaque handle. File/fd/flush protocols without intrinsic drops are outside
    # this census; their absence is not a claim of conformance.
    exceptions = {("tkinter/font.py", "Font")}
    behavioral_owners = {row[:2] for row in _HANDLE_FINALIZERS}
    behavioral_owners.update(row[:2] for row in _CLOSE_DROP_OWNERS)
    behavioral_owners.update({("subprocess.py", "Popen"), ("dbm/dumb.py", "_Database")})
    assert set(rows) - exceptions == behavioral_owners
    assert not [row for row in violations if row[:2] not in exceptions]


@pytest.mark.parametrize("rejected_field", ["_handle", "_stdout_reader"])
def test_popen_preserves_attribute_read_failure_policy_before_detaching(rejected_field):
    fields = (
        "_stdin_stream",
        "_stdout_stream",
        "_stderr_stream",
        "_stdout_reader",
        "_stderr_reader",
        "_handle",
    )
    calls = []
    error = RuntimeError("Popen attribute witness")
    owner, namespace = _cleanup_owner(
        "subprocess.py",
        "Popen",
        {
            "_MOLT_STREAM_DROP": calls.append,
            "_MOLT_STREAM_READER_DROP": calls.append,
            "_MOLT_PROCESS_DROP": calls.append,
        },
        include_close=False,
    )

    class RejectReads(owner):
        reject = True

        def __getattribute__(self, name):
            if name == rejected_field and object.__getattribute__(self, "reject"):
                raise error
            return object.__getattribute__(self, name)

    instance = RejectReads()
    for handle, field in enumerate(fields):
        setattr(instance, field, handle)
    namespace.clear()
    if rejected_field == "_handle":
        with pytest.raises(RuntimeError) as raised:
            instance.__del__()
        assert raised.value is error
    else:
        instance.__del__()
    assert calls == []
    assert [object.__getattribute__(instance, field) for field in fields] == list(
        range(6)
    )
    instance.reject = False
    instance.__del__()
    assert calls == list(range(6))


def test_popen_partial_constructor_does_not_publish_missing_fields_during_retirement():
    fields = (
        "_stdin_stream",
        "_stdout_stream",
        "_stderr_stream",
        "_stdout_reader",
        "_stderr_reader",
        "_handle",
    )
    calls = []
    instance = None

    def drop(handle):
        assert all(getattr(instance, field, None) is None for field in fields)
        calls.append(handle)
        instance.__del__()

    owner, namespace = _cleanup_owner(
        "subprocess.py",
        "Popen",
        {
            "_MOLT_STREAM_DROP": drop,
            "_MOLT_STREAM_READER_DROP": drop,
            "_MOLT_PROCESS_DROP": drop,
        },
        include_close=False,
    )
    instance = owner()
    instance._handle = 73
    instance._stdin_stream = 0
    namespace.clear()
    instance.__del__()
    assert calls == [0, 73]
    assert instance.__dict__ == {"_handle": None, "_stdin_stream": None}
