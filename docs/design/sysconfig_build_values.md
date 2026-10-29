# sysconfig in compiled Molt programs

**Status:** implemented (HF-273 slice: `sysconfig`, `_sysconfig`, `_osx_support`)
**Owner:** stdlib

## Decision

A compiled Molt program runs CPython's `sysconfig` code. The module is a port
of CPython 3.14's `Lib/sysconfig/__init__.py`. Each difference between 3.12,
3.13, 3.14 and 3.15 is a `sys.version_info` gate in the one source, as
`glob`, `threading` and `_opcode_metadata` gate theirs.

The values describe the running Molt binary. They do not copy any CPython
build. One function, `sysconfig._get_sysconfigdata()`, derives the binary's
build variables from the runtime authorities that own each fact. `_sysconfig`
(3.13 and later) selects CPython's key set from that function. A Makefile or
`pyconfig.h` variable that Molt has no value for is absent, so
`get_config_var()` returns `None`, as CPython does for an unknown name.

## Prior art

| System | Where the build values come from |
| --- | --- |
| CPython, POSIX | `_generate_posix_vars()` parses the installed Makefile and `pyconfig.h` at build time into `_sysconfigdata_<abiflags>_<platform>_<multiarch>.py`; `_init_posix()` imports it. |
| CPython, Windows | `_init_non_posix()` computes the values. From 3.13 the C module `_sysconfig` (`Modules/_sysconfig.c`) reports what the compiler knew: `EXT_SUFFIX`, `SOABI`, `Py_GIL_DISABLED`, and from 3.14 `Py_DEBUG`. 3.15 adds `_sysconfig.get_platform()`. |
| PyPy | `lib_pypy/_sysconfigdata.py` computes its values when imported, from `_imp.extension_suffixes()`, `sys.implementation._multiarch` and `struct`. |
| GraalPy, Pyodide | Ship their own data for their own runtime; Pyodide generates `_sysconfigdata__emscripten_wasm32-emscripten` for its cross target. |
| Nuitka, PyInstaller | Freeze the building CPython's own `_sysconfigdata` into the bundle, so the values describe that CPython. |

Molt follows PyPy: the values come from the running interpreter. A generated
`_sysconfigdata` table would be a second authority for facts the runtime
already owns, and Nuitka's frozen copy would describe a CPython build that is
not the program. A compiled program also resolves imports when it is built,
so CPython's import of a module whose name is computed at run time has no
module to find.

## Facts and their authorities

| Variables | Authority |
| --- | --- |
| `py_version`, `py_version_short`, `py_version_nodot`, `VERSION` | the target Python version (`sys.version_info`, `sys.version`) |
| `ABIFLAGS`, `abiflags`, `Py_DEBUG` (`d`), `Py_GIL_DISABLED` (`t`, 3.13+), `abi_thread` (3.13+) | the runtime's ABI flags (`molt_sys_abiflags`, published on every platform; `sys.abiflags` is POSIX only) |
| `EXT_SUFFIX` | `_imp.extension_suffixes()[0]` when the list is not empty, CPython's rule for a build without dynamic loading (GH-99201) |
| `MACHDEP` | `sys.platform` |
| `prefix`, `base`, `installed_base`, `projectbase`, `srcdir`, path schemes | `sys.prefix`, `sys.base_prefix`, `sys.executable`, computed as CPython computes them |
| `get_platform()` on Windows | the runtime's target architecture (`molt_platform_processor`) |
| `get_platform()` on macOS | `_osx_support.get_platform_osx()`; with no `MACOSX_DEPLOYMENT_TARGET` it uses the running system's version, CPython's own fallback |

`_imp.extension_suffixes()` is empty because a compiled program links its
source-built extensions when it is built and loads none at run time. So
`EXT_SUFFIX` and `SOABI` are absent. Compiler, linker and install variables
(`CC`, `CFLAGS`, `CONFIG_ARGS`, `LIBDIR`, `MULTIARCH`, `TZPATH` on POSIX,
`WITH_DOC_STRINGS`) have no Molt value and are absent too. `_installation_is_relocated()`
and the 3.14 cross-build prefixes read a Makefile that a binary does not have,
so the port leaves them out.

## Host reads

`sysconfig` and `_osx_support` read the host only through `os` and `open`:
`os.environ` (`env.read`), `realpath`, `isfile` and the macOS
`SystemVersion.plist` (`fs.read`), and `os.uname()` for `get_platform()`. Those
modules own the capability gates. A denied read raises the gate's error, and
`_osx_support` treats a denied plist read as a host with no version file, as
CPython treats any `OSError`.

## Version gates

- Scheme templates: 3.12 spells `python{py_version_short}`; 3.13 spells
  `{implementation_lower}{py_version_short}{abi_thread}`.
- `_getuserbase()`: 3.13 adds iOS, tvOS and watchOS; 3.14 reads
  `_PYTHON_HOST_PLATFORM`.
- `is_python_build(check_home)`: deprecated through 3.14, removed in 3.15.
- `_init_config_vars()`: 3.13 adds `implementation`, `implementation_lower` and
  `abi_thread`; 3.14 reads the build variables first and invalidates the cache
  when `sys.exec_prefix` changes too.
- `parse_config_h()`: 3.14 accepts `_Py_` names; 3.15 unquotes values.
- `get_platform()`: 3.13 adds Android and iOS; 3.14 drops empty parts.
- `expand_makefile_vars()`: deprecated from 3.14.
- `_sysconfig`: absent before 3.13, as in CPython.

## Open items

- `sys_abiflags()` in the runtime returns `""` even when the runtime is built
  with its `free-threaded` feature, and `sys.flags.gil` is always 1. Then
  `Py_GIL_DISABLED` would read 0 for a free-threaded runtime. The fix belongs
  in the ABI-flag authority; `sysconfig` follows it unchanged.
- The binary's macOS deployment target is a build fact the runtime does not
  publish, so `get_platform()` names the running system's version.
- `importlib.machinery.EXTENSION_SUFFIXES` lists `.so`/`.dylib` while
  `_imp.extension_suffixes()` is empty; CPython defines the first as the second.
- `sysconfig.__main__` (`python -m sysconfig`) is still a generated stub.
- Windows behavior is not yet proven on a Windows host.
