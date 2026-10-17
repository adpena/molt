from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

from tests.executable_test_support import native_executable_name
from tests.native_process_guard import run_native_test_process


_HELPER_PATH = Path(__file__).with_name("test_native_import_bootstrap_regressions.py")
_HELPER_SPEC = importlib.util.spec_from_file_location(
    "_molt_native_import_bootstrap_helpers_for_public_api",
    _HELPER_PATH,
)
assert _HELPER_SPEC is not None
assert _HELPER_SPEC.loader is not None
_HELPERS = importlib.util.module_from_spec(_HELPER_SPEC)
sys.modules[_HELPER_SPEC.name] = _HELPERS
_HELPER_SPEC.loader.exec_module(_HELPERS)

ARTIFACT_ROOT = _HELPERS.ARTIFACT_ROOT
_build_native_binary_with_env = _HELPERS._build_native_binary_with_env
_build_and_run_with_env = _HELPERS._build_and_run_with_env


def test_native_importlib_public_api_validation_matches_cpython(
    tmp_path: Path,
) -> None:
    # Argument validation and resolve_name are pure operations. An ambient
    # developer grant must not hide a premature filesystem search.
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import importlib\n"
            "import importlib.util\n"
            "\n"
            "def show(label, func):\n"
            "    try:\n"
            "        value = func()\n"
            "    except BaseException as exc:\n"
            "        print(label, type(exc).__name__, str(exc))\n"
            "    else:\n"
            "        print(label, 'OK', repr(value))\n"
            "\n"
            "show('import-module-name-nonstr', lambda: importlib.import_module(1))\n"
            "show('import-module-relative-package-none', lambda: importlib.import_module('.x', None))\n"
            "show('import-module-relative-package-missing', lambda: importlib.import_module('.x'))\n"
            "show('import-module-relative-package-nonstr', lambda: importlib.import_module('.x', 1))\n"
            "show('import-module-relative-package-empty', lambda: importlib.import_module('.x', ''))\n"
            "show('import-module-beyond-top', lambda: importlib.import_module('..x', 'pkg'))\n"
            "show('import-module-empty-name', lambda: importlib.import_module(''))\n"
            "show('util-name-nonstr', lambda: importlib.util.resolve_name(1, None))\n"
            "show('util-relative-package-none', lambda: importlib.util.resolve_name('.x', None))\n"
            "show('util-relative-package-nonstr', lambda: importlib.util.resolve_name('.x', 1))\n"
            "show('util-relative-package-empty', lambda: importlib.util.resolve_name('.x', ''))\n"
            "show('util-beyond-top', lambda: importlib.util.resolve_name('..x', 'pkg'))\n"
            "show('util-empty-name', lambda: importlib.util.resolve_name('', None))\n"
            "show('util-relative-empty', lambda: importlib.util.resolve_name('.', 'pkg'))\n"
        ),
        native_executable_name("importlib_public_api_validation"),
        source_relpath="importlib_public_api_validation.py",
        session_id=_HELPERS.NATIVE_BOOTSTRAP_SESSION_ID,
        cache_dir=ARTIFACT_ROOT / ".molt_cache",
        backend="cranelift",
        extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
        extra_build_args=["--no-trusted"],
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "import-module-name-nonstr AttributeError 'int' object has no attribute 'startswith'",
        "import-module-relative-package-none TypeError the 'package' argument is required to perform a relative import for '.x'",
        "import-module-relative-package-missing TypeError the 'package' argument is required to perform a relative import for '.x'",
        "import-module-relative-package-nonstr TypeError __package__ not set to a string",
        "import-module-relative-package-empty TypeError the 'package' argument is required to perform a relative import for '.x'",
        "import-module-beyond-top ImportError attempted relative import beyond top-level package",
        "import-module-empty-name ValueError Empty module name",
        "util-name-nonstr AttributeError 'int' object has no attribute 'startswith'",
        "util-relative-package-none ImportError no package specified for '.x' (required for relative module names)",
        "util-relative-package-nonstr AttributeError 'int' object has no attribute 'rsplit'",
        "util-relative-package-empty ImportError no package specified for '.x' (required for relative module names)",
        "util-beyond-top ImportError attempted relative import beyond top-level package",
        "util-empty-name OK ''",
        "util-relative-empty OK 'pkg'",
    ]


def test_native_importlib_discovery_preserves_capability_policy(tmp_path: Path) -> None:
    # CPython 3.12's _resolve_name('', package, 1) returns package; importing
    # that name then calls finders. Denial is Molt policy, not CPython absence.
    source = """import importlib
import importlib.machinery
import sys

# This policy control owns a real filesystem search. An empty public path
# legitimately ends discovery without any I/O or filesystem authorization.
assert sys.meta_path[-1] is importlib.machinery.PathFinder
sys.path[:] = ['.']
missing = '_molt_public_import_policy_absent_39c5b8'
try:
    importlib.import_module('.', missing)
except ImportError as exc:
    print('import-error', type(exc).__name__, str(exc))
except PermissionError as exc:
    print('permission-error', type(exc).__name__, str(exc))
print('missing-unpublished', missing not in sys.modules)

# The same name must import from the public cache without host discovery.
cached = object()
sys.modules[missing] = cached
print('cache-hit', importlib.import_module('.', missing) is cached)
"""
    manifest = tmp_path / "policy.toml"
    empty_path = tmp_path / "no-python-on-path"
    empty_path.mkdir()
    for policy, allowed in (
        ("allow = []\n", False),
        ('allow = ["fs.read"]\n', True),
        ('allow = ["fs.read"]\ndeny = ["fs.read"]\n', False),
    ):
        manifest.write_text(
            '[manifest]\nversion = "2.0"\n[capabilities]\n'
            + policy
            + '[audit]\nenabled = true\nsink = "jsonl"\noutput = "stderr"\n',
            encoding="utf-8",
        )
        # Same source, output and cache: changing policy reuses compiled code,
        # while the current manifest alone owns the emitted stub's grants.
        binary, env = _build_native_binary_with_env(
            tmp_path,
            source,
            native_executable_name("importlib_public_policy"),
            source_relpath="importlib_public_policy.py",
            session_id=_HELPERS.NATIVE_BOOTSTRAP_SESSION_ID,
            cache_dir=ARTIFACT_ROOT / ".molt_cache",
            backend="cranelift",
            extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
            extra_build_args=[
                "--no-trusted",
                "--cache",
                "--cache-dir",
                str(tmp_path / "code-cache"),
                "--capability-manifest",
                str(manifest),
            ],
        )
        # Adversarial ambient authority must not override the built policy.
        env.update(
            PATH=str(empty_path),
            PYTHONPATH="",
            PYTHONHOME="",
            MOLT_CAPABILITY_TIER="none" if allowed else "full",
            MOLT_CAPABILITIES="" if allowed else "fs.read",
        )
        run = run_native_test_process(
            [str(binary)],
            cwd=tmp_path,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert run.returncode == 0, policy + run.stdout + run.stderr
        missing = "_molt_public_import_policy_absent_39c5b8"
        discovery = (
            f"import-error ModuleNotFoundError No module named '{missing}'"
            if allowed
            else "permission-error PermissionError missing [fs.read] capabilities for 'os.stat'"
        )
        assert run.stdout.splitlines() == [
            discovery,
            "missing-unpublished True",
            "cache-hit True",
        ], policy + run.stdout + run.stderr
        decisions = [
            json.loads(line) for line in run.stderr.splitlines() if line.startswith("{")
        ]
        reads = [event for event in decisions if event["capability"] == "fs.read"]
        assert reads, policy + run.stderr
        assert all(
            event["decision"]["status"] == ("allowed" if allowed else "denied")
            for event in reads
        ), policy + run.stderr


def test_native_importlib_import_module_joined_singleton_name_returns_module(
    tmp_path: Path,
) -> None:
    run = _build_and_run_with_env(
        tmp_path,
        (
            "import importlib\n"
            "\n"
            "def _module_name(parts: tuple[str, ...]) -> str:\n"
            "    return '.'.join(parts)\n"
            "\n"
            "def _load(parts: tuple[str, ...]):\n"
            "    return importlib.import_module(_module_name(parts))\n"
            "\n"
            "math_mod = _load(('math',))\n"
            "util_mod = _load(('importlib', 'util'))\n"
            "print(math_mod.__name__)\n"
            "print(util_mod.__name__)\n"
            "print(math_mod is importlib.import_module('math'))\n"
            "print(util_mod is importlib.import_module('importlib.util'))\n"
        ),
        native_executable_name("importlib_import_module_joined_singleton_name"),
        source_relpath="importlib_import_module_joined_singleton_name.py",
        session_id="pytest-native-importlib-public-api-joined-name",
        cache_dir=ARTIFACT_ROOT / ".molt_cache",
        backend="cranelift",
        extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
        extra_build_args=[
            "--no-trusted",
            "--stdlib-profile",
            "full",
        ],
    )

    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.strip().splitlines() == [
        "math",
        "importlib.util",
        "True",
        "True",
    ]


def test_native_importlib_finders_follow_live_public_protocol(tmp_path: Path) -> None:
    # Pinned CPython 3.12.13 util.find_spec/_find_spec/PathFinder own this
    # protocol transcript. The final extension denial is Molt admission policy.
    # No filesystem grant can hide a path fallback or hook call.
    source = """import importlib
import importlib.machinery
import importlib.util
import sys
import types


def show_error(label, call):
    try:
        call()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, 'no-error')


hook_calls = []


def forbidden_hook(path):
    hook_calls.append(path)
    raise AssertionError('meta-only discovery invoked a path hook')


class LiveFinder:
    def __init__(self):
        self.calls = 0
        self.result = None

    def find_spec(self, fullname, path=None, target=None):
        self.calls += 1
        return self.result


sys.path[:] = ['molt://meta-only']
sys.path_hooks[:] = [forbidden_hook]
sys.path_importer_cache.clear()
sys.meta_path[:] = []
print('empty-meta', importlib.util.find_spec('_molt_hf297_meta_empty') is None)
finder = LiveFinder()
sys.meta_path[:] = [finder]
name = '_molt_hf297_meta_live'
print('live-none', importlib.util.find_spec(name) is None)
first = importlib.machinery.ModuleSpec(name, None, origin='live://first')
finder.result = first
print('live-first', importlib.util.find_spec(name) is first)
second = importlib.machinery.ModuleSpec(name, None, origin='live://second')
finder.result = second
print('live-second', importlib.util.find_spec(name) is second)
print('live-calls', finder.calls)
print('meta-hook-calls', len(hook_calls))
print('meta-public-cache', len(sys.path_importer_cache))


class RaisingMetaFinder:
    def __init__(self):
        self.calls = 0

    def find_spec(self, fullname, path=None, target=None):
        self.calls += 1
        raise TypeError('meta-finder-body')


raising_meta = RaisingMetaFinder()
later_meta = LiveFinder()
sys.meta_path[:] = [raising_meta, later_meta]
show_error('meta-type-error', lambda: importlib.util.find_spec('_molt_hf297_meta_error'))
print('meta-error-calls', raising_meta.calls, later_meta.calls)


class RaisingPathFinder:
    def __init__(self):
        self.calls = 0

    def find_spec(self, fullname, target=None):
        self.calls += 1
        raise TypeError('path-finder-body')


raising_path = RaisingPathFinder()
path_hook_calls = []


def raising_path_hook(path):
    path_hook_calls.append(path)
    return raising_path


sys.meta_path[:] = [importlib.machinery.PathFinder, later_meta]
sys.path_hooks[:] = [raising_path_hook]
sys.path[:] = ['molt://path-error']
sys.path_importer_cache.clear()
show_error('path-type-error', lambda: importlib.util.find_spec('_molt_hf297_path_error'))
print('path-error-calls', raising_path.calls, later_meta.calls, len(path_hook_calls))
print('path-error-cache', sys.path_importer_cache['molt://path-error'] is raising_path)


parent_name = '_molt_hf297_parent'
child_name = parent_name + '.child'
parent_path = ['molt://exact-parent-path']
parent_events = []


class ParentLoader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        parent_events.append('executed')
        module.__path__ = parent_path
        module.executed = True


parent_spec = importlib.machinery.ModuleSpec(parent_name, ParentLoader(), is_package=True)
child_spec = importlib.machinery.ModuleSpec(child_name, None, origin='parent://child')


class ParentFinder:
    def find_spec(self, fullname, path=None, target=None):
        if fullname == parent_name:
            parent_events.append(path is None)
            return parent_spec
        if fullname == child_name:
            parent_events.append(path is parent_path)
            return child_spec
        return None


sys.meta_path[:] = [ParentFinder()]
sys.modules.pop(parent_name, None)
sys.modules.pop(child_name, None)
print('parent-before', parent_name in sys.modules)
print('parent-child-spec', importlib.util.find_spec(child_name) is child_spec)
parent = sys.modules[parent_name]
print('parent-execution', parent.executed, parent.__path__ is parent_path)
print('parent-events', parent_events)
print('child-not-imported', child_name not in sys.modules)


cached_name = '_molt_hf297_cached'
sys.modules[cached_name] = None
print('module-none', importlib.util.find_spec(cached_name) is None)
cached = types.ModuleType(cached_name)
sys.modules[cached_name] = cached
del cached.__spec__
show_error('module-missing-spec', lambda: importlib.util.find_spec(cached_name))
cached.__spec__ = None
show_error('module-none-spec', lambda: importlib.util.find_spec(cached_name))
cached_spec = object()
cached.__spec__ = cached_spec
print('module-spec-identity', importlib.util.find_spec(cached_name) is cached_spec)


none_hook_calls = []
later_hook_calls = []


def none_hook(path):
    none_hook_calls.append(path)
    return None


def later_hook(path):
    later_hook_calls.append(path)
    raise AssertionError('a successful None hook must terminate hook search')


sys.meta_path[:] = [importlib.machinery.PathFinder]
sys.path_hooks[:] = [none_hook, later_hook]
sys.path[:] = ['molt://none-hook']
sys.path_importer_cache.clear()
print('none-hook-first', importlib.util.find_spec('_molt_hf297_hook_none') is None)
print('none-hook-second', importlib.util.find_spec('_molt_hf297_hook_none') is None)
print('none-hook-calls', len(none_hook_calls), len(later_hook_calls))
print('none-hook-cache', 'molt://none-hook' in sys.path_importer_cache,
      sys.path_importer_cache['molt://none-hook'] is None)

hook_error_calls = []


def raising_hook(path):
    hook_error_calls.append(path)
    raise KeyError('path-hook-body')


sys.path_hooks[:] = [raising_hook, later_hook]
sys.path[:] = ['molt://hook-error']
sys.path_importer_cache.clear()
show_error('hook-key-error', lambda: importlib.util.find_spec('_molt_hf297_hook_error'))
print('hook-error-calls', len(hook_error_calls), len(later_hook_calls))
print('hook-error-uncached', 'molt://hook-error' not in sys.path_importer_cache)

# Admission applies to the selected public spec, not just the provisional
# callback result. The uncreated extension path must never be inspected.
selected_name = '_molt_hf297_selected_extension'
selected = types.ModuleType(selected_name)
extension_path = '/__molt_hf297_uncreated_extension__.so'
selected_spec = importlib.machinery.ModuleSpec(
    selected_name,
    importlib.machinery.ExtensionFileLoader(selected_name, extension_path),
    origin=extension_path,
)
selected.__spec__ = selected_spec


class PublishingExtensionFinder:
    def find_spec(self, fullname, path=None, target=None):
        sys.modules[fullname] = selected
        return importlib.machinery.ModuleSpec(fullname, None, origin='memory://unused')


sys.meta_path[:] = [PublishingExtensionFinder()]
show_error('selected-extension', lambda: importlib.util.find_spec(selected_name))
print('selected-extension-cache', sys.modules[selected_name] is selected,
      selected.__spec__ is selected_spec)
"""
    binary, env = _build_native_binary_with_env(
        tmp_path,
        source,
        native_executable_name("importlib_live_finder_protocol"),
        source_relpath="importlib_live_finder_protocol.py",
        session_id=_HELPERS.NATIVE_BOOTSTRAP_SESSION_ID,
        cache_dir=ARTIFACT_ROOT / ".molt_cache",
        backend="cranelift",
        extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
        extra_build_args=["--no-trusted", "--stdlib-profile", "full"],
    )
    empty_path = tmp_path / "no-python-on-path"
    empty_path.mkdir()
    env.update(PATH=str(empty_path), PYTHONPATH="", PYTHONHOME="")
    run = run_native_test_process(
        [str(binary)],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.splitlines() == [
        "empty-meta True",
        "live-none True",
        "live-first True",
        "live-second True",
        "live-calls 3",
        "meta-hook-calls 0",
        "meta-public-cache 0",
        "meta-type-error TypeError meta-finder-body",
        "meta-error-calls 1 0",
        "path-type-error TypeError path-finder-body",
        "path-error-calls 1 0 1",
        "path-error-cache True",
        "parent-before False",
        "parent-child-spec True",
        "parent-execution True True",
        "parent-events [True, 'executed', True]",
        "child-not-imported True",
        "module-none True",
        "module-missing-spec ValueError _molt_hf297_cached.__spec__ is not set",
        "module-none-spec ValueError _molt_hf297_cached.__spec__ is None",
        "module-spec-identity True",
        "none-hook-first True",
        "none-hook-second True",
        "none-hook-calls 1 0",
        "none-hook-cache True True",
        "hook-key-error KeyError 'path-hook-body'",
        "hook-error-calls 1 0",
        "hook-error-uncached True",
        "selected-extension PermissionError missing [fs.read] capabilities for 'importlib.spec.extension'",
        "selected-extension-cache True True",
    ]


def test_native_importlib_raw_filesystem_finders_obey_capability_policy(
    tmp_path: Path,
) -> None:
    import zipfile

    # CPython independently finds these actual local files through both raw
    # finders. PermissionError is Molt policy; a named stdlib spelling must
    # require the same authority as an otherwise unknown source filename.
    source = """import importlib.machinery
import os.path
import sys
import zipimport


default_hooks = list(sys.path_hooks)
entry = sys.argv[1]
details = (importlib.machinery.SourceFileLoader, importlib.machinery.SOURCE_SUFFIXES)
sys.meta_path[:] = [importlib.machinery.PathFinder]
sys.path_hooks[:] = [importlib.machinery.FileFinder.path_hook(details)]
sys.path[:] = [entry]

for kind in ('file', 'path'):
    for fullname in ('math', '_molt_hf297_disk_module'):
        sys.path_importer_cache.clear()
        try:
            if kind == 'file':
                finder = importlib.machinery.FileFinder(entry, details)
                spec = finder.find_spec(fullname)
            else:
                spec = importlib.machinery.PathFinder.find_spec(fullname, [entry])
        except PermissionError as exc:
            print(kind, fullname, type(exc).__name__, str(exc))
        else:
            print(kind, fullname, 'source', spec is not None,
                  spec is not None and spec.origin == os.path.join(entry, fullname + '.py'))

# Exercise actual default archive admission before later directory/custom hooks.
archive = os.path.join(entry, 'cache.zip', 'app.zip')
prefix_entry = os.path.join(archive, 'nested')
directory_entry = os.path.join(entry, 'plugins.zip')
invalid_entry = os.path.join(entry, 'data.zip')
archive_name = '_molt_hf297_archive_module'
directory_name = '_molt_hf297_directory_module'
fallback_name = '_molt_hf297_invalid_archive_fallback'

for label, path, expected_prefix in (
    ('zip-archive', archive, ''),
    ('zip-prefix', prefix_entry, 'nested' + os.path.sep),
    ('zip-directory', directory_entry, ''),
    ('zip-invalid', invalid_entry, ''),
):
    try:
        importer = zipimport.zipimporter(path)
    except PermissionError as exc:
        print(label, type(exc).__name__, str(exc))
    except zipimport.ZipImportError as exc:
        print(label, type(exc).__name__)
    else:
        print(label, 'accepted', importer.archive == archive,
              importer.prefix == expected_prefix)


class FallbackLoader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        raise AssertionError('find_spec initialized the custom fallback module')


class FallbackFinder:
    def find_spec(self, fullname, target=None):
        if fullname == fallback_name:
            return importlib.machinery.ModuleSpec(
                fullname, FallbackLoader(), origin='hook://invalid-archive')
        return None


fallback_calls = []


def fallback_hook(path):
    if path == invalid_entry:
        fallback_calls.append(path)
        return FallbackFinder()
    raise ImportError(path)


sys.path_hooks[:] = [*default_hooks, fallback_hook]
for label, path, name, origin in (
    ('zip-path', prefix_entry, archive_name,
     os.path.join(archive, 'nested', archive_name + '.py')),
    ('zip-directory-path', directory_entry, directory_name,
     os.path.join(directory_entry, directory_name + '.py')),
    ('zip-invalid-path', invalid_entry, fallback_name, 'hook://invalid-archive'),
):
    sys.path_importer_cache.clear()
    try:
        spec = importlib.machinery.PathFinder.find_spec(name, [path])
    except PermissionError as exc:
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, 'spec', spec is not None,
              spec is not None and spec.origin == origin)
print('zip-fallback-hooks', len(fallback_calls))
print('zip-modules-unpublished', archive_name not in sys.modules,
      directory_name not in sys.modules, fallback_name not in sys.modules)
"""
    discovery_root = tmp_path / "finder-owned-sources"
    discovery_root.mkdir()
    for name in ("math", "_molt_hf297_disk_module"):
        (discovery_root / (name + ".py")).write_text("value = 297\n", encoding="utf-8")
    archive_parent = discovery_root / "cache.zip"
    archive_parent.mkdir()
    with zipfile.ZipFile(archive_parent / "app.zip", "w") as handle:
        handle.writestr(
            "nested/_molt_hf297_archive_module.py",
            "raise AssertionError('find_spec initialized archive source')\n",
        )
    directory_entry = discovery_root / "plugins.zip"
    directory_entry.mkdir()
    (directory_entry / "_molt_hf297_directory_module.py").write_text(
        "raise AssertionError('find_spec initialized directory source')\n",
        encoding="utf-8",
    )
    (discovery_root / "data.zip").write_bytes(b"not a ZIP archive\n")
    manifest = tmp_path / "finder-policy.toml"
    empty_path = tmp_path / "no-python-on-path"
    empty_path.mkdir()
    for policy, expected in (
        (
            "allow = []\n",
            [
                "file math PermissionError missing [fs.read] capabilities for 'importlib.filefinder.find_spec'",
                "file _molt_hf297_disk_module PermissionError missing [fs.read] capabilities for 'importlib.filefinder.find_spec'",
                "path math PermissionError missing [fs.read] capabilities for 'os.stat'",
                "path _molt_hf297_disk_module PermissionError missing [fs.read] capabilities for 'os.stat'",
                "zip-archive PermissionError missing [fs.read] capabilities for 'importlib.zip.archive_path'",
                "zip-prefix PermissionError missing [fs.read] capabilities for 'importlib.zip.archive_path'",
                "zip-directory PermissionError missing [fs.read] capabilities for 'importlib.zip.archive_path'",
                "zip-invalid PermissionError missing [fs.read] capabilities for 'importlib.zip.archive_path'",
                "zip-path PermissionError missing [fs.read] capabilities for 'importlib.zip.archive_path'",
                "zip-directory-path PermissionError missing [fs.read] capabilities for 'importlib.zip.archive_path'",
                "zip-invalid-path PermissionError missing [fs.read] capabilities for 'importlib.zip.archive_path'",
                "zip-fallback-hooks 0",
                "zip-modules-unpublished True True True",
            ],
        ),
        (
            'allow = ["fs.read"]\n',
            [
                "file math source True True",
                "file _molt_hf297_disk_module source True True",
                "path math source True True",
                "path _molt_hf297_disk_module source True True",
                "zip-archive accepted True True",
                "zip-prefix accepted True True",
                "zip-directory ZipImportError",
                "zip-invalid ZipImportError",
                "zip-path spec True True",
                "zip-directory-path spec True True",
                "zip-invalid-path spec True True",
                "zip-fallback-hooks 1",
                "zip-modules-unpublished True True True",
            ],
        ),
    ):
        manifest.write_text(
            '[manifest]\nversion = "2.0"\n[capabilities]\n' + policy,
            encoding="utf-8",
        )
        # One source/output/cache owns the cold compilation; only the current
        # manifest changes between denied and real authorized discovery.
        binary, env = _build_native_binary_with_env(
            tmp_path,
            source,
            native_executable_name("importlib_raw_filesystem_finders"),
            source_relpath="importlib_raw_filesystem_finders.py",
            session_id=_HELPERS.NATIVE_BOOTSTRAP_SESSION_ID,
            cache_dir=ARTIFACT_ROOT / ".molt_cache",
            backend="cranelift",
            extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
            extra_build_args=[
                "--no-trusted",
                "--stdlib-profile",
                "full",
                "--cache",
                "--cache-dir",
                str(tmp_path / "code-cache"),
                "--capability-manifest",
                str(manifest),
            ],
        )
        env.update(PATH=str(empty_path), PYTHONPATH="", PYTHONHOME="")
        run = run_native_test_process(
            [str(binary), str(discovery_root)],
            cwd=tmp_path,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        assert run.returncode == 0, policy + run.stdout + run.stderr
        assert run.stdout.splitlines() == expected, policy + run.stdout + run.stderr


def test_native_find_spec_describes_compiled_catalog_without_initializing_child(
    tmp_path: Path,
) -> None:
    source = """import importlib.machinery
import importlib.util
import sys

print('package-metadata', importlib.__path__ is
      importlib.__spec__.submodule_search_locations)
print('default-finders',
      sys.meta_path[0] is importlib.machinery.BuiltinImporter,
      sys.meta_path[-1] is importlib.machinery.PathFinder)
print('initially-absent', '_molt_catalog_pkg' not in sys.modules,
      '_molt_catalog_pkg.child' not in sys.modules)
spec = importlib.util.find_spec('_molt_catalog_pkg')
print('package-spec', spec.name, spec.loader is importlib.machinery._MOLT_LOADER,
      bool(spec.submodule_search_locations))
print('description-unpublished', '_molt_catalog_pkg' not in sys.modules)
child_spec = importlib.util.find_spec('_molt_catalog_pkg.child')
print('child-spec', child_spec.name,
      child_spec.loader is importlib.machinery._MOLT_LOADER,
      child_spec.submodule_search_locations is None)
print('parent-ready', sys.modules['_molt_catalog_pkg'].value == 297)
print('child-unpublished', '_molt_catalog_pkg.child' not in sys.modules)
import _molt_catalog_pkg.child
print('child-ready', _molt_catalog_pkg.child.value == 298)
"""
    source += (
        Path(__file__).resolve().parent
        / "differential/stdlib/importlib_find_spec_invalidate_caches_intrinsic.py"
    ).read_text(encoding="utf-8")
    binary, env = _build_native_binary_with_env(
        tmp_path,
        source,
        native_executable_name("importlib_compiled_catalog"),
        source_relpath="importlib_compiled_catalog.py",
        session_id=_HELPERS.NATIVE_BOOTSTRAP_SESSION_ID,
        cache_dir=ARTIFACT_ROOT / ".molt_cache",
        backend="cranelift",
        extra_files={
            "_molt_catalog_pkg/__init__.py": "print('package-exec')\nvalue = 297\n",
            "_molt_catalog_pkg/child.py": "print('child-exec')\nvalue = 298\n",
        },
        extra_env={"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""},
        extra_build_args=["--no-trusted"],
    )
    empty_path = tmp_path / "no-python-on-path"
    empty_path.mkdir()
    env.update(PATH=str(empty_path), PYTHONPATH="", PYTHONHOME="")
    run = run_native_test_process(
        [str(binary)], cwd=tmp_path, env=env, capture_output=True, text=True, timeout=60
    )
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.splitlines() == [
        "package-metadata True",
        "default-finders True True",
        "initially-absent True True",
        "package-spec _molt_catalog_pkg True True",
        "description-unpublished True",
        "package-exec",
        "child-spec _molt_catalog_pkg.child True True",
        "parent-ready True",
        "child-unpublished True",
        "child-exec",
        "child-ready True",
        "invalidate://before",
        "invalidate://before",
        "invalidate://after",
        "1",
        "2",
        "True",
        "pathfinder-cache-invalidation True True True True",
        "invalidation-errors True",
        "import-root-cases 8" if sys.platform == "win32" else "import-root-cases 4",
    ]
