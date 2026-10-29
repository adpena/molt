"""Access to Python's configuration information."""

# Ported from CPython 3.14's Lib/sysconfig/__init__.py, with each 3.12-3.15
# difference gated on the target version. A compiled Molt program has no
# Makefile, pyconfig.h or generated _sysconfigdata module: _get_sysconfigdata()
# derives the binary's build variables from the runtime's own authorities, and
# a variable Molt has no value for is absent, so get_config_var() returns None.
# See docs/design/sysconfig_build_values.md.

import os
import sys
import threading
from os.path import realpath

from _intrinsics import require_intrinsic as _require_intrinsic

# The runtime's ABI flags, published on every platform (sys.abiflags is POSIX
# only), and its target architecture.
_MOLT_SYS_ABIFLAGS = _require_intrinsic("molt_sys_abiflags")
_MOLT_TARGET_MACHINE = _require_intrinsic("molt_platform_processor")

__all__ = [
    'get_config_h_filename',
    'get_config_var',
    'get_config_vars',
    'get_makefile_filename',
    'get_path',
    'get_path_names',
    'get_paths',
    'get_platform',
    'get_python_version',
    'get_scheme_names',
    'parse_config_h',
]

# Keys for get_config_var() that are never converted to Python integers.
_ALWAYS_STR = {
    'MACOSX_DEPLOYMENT_TARGET',
}
if sys.version_info >= (3, 13):
    _ALWAYS_STR.add('IPHONEOS_DEPLOYMENT_TARGET')

# Python 3.13 spells the interpreter directory through the implementation
# name and the free-threading suffix; 3.12 spells it "python".
if sys.version_info >= (3, 13):
    _LIB = '{implementation_lower}{py_version_short}{abi_thread}'
    _INCLUDE = '{implementation_lower}{py_version_short}{abiflags}'
    _HOME = '{implementation_lower}'
    _NT_USER = '{implementation}{py_version_nodot_plat}'
    _USER_INCLUDE = '{implementation_lower}{py_version_short}{abi_thread}'
    _FRAMEWORK_INCLUDE = '{implementation_lower}{py_version_short}'
else:
    _LIB = 'python{py_version_short}'
    _INCLUDE = 'python{py_version_short}{abiflags}'
    _HOME = 'python'
    _NT_USER = 'Python{py_version_nodot_plat}'
    _USER_INCLUDE = 'python{py_version_short}'
    _FRAMEWORK_INCLUDE = 'python{py_version_short}'

_INSTALL_SCHEMES = {
    'posix_prefix': {
        'stdlib': '{installed_base}/{platlibdir}/' + _LIB,
        'platstdlib': '{platbase}/{platlibdir}/' + _LIB,
        'purelib': '{base}/lib/' + _LIB + '/site-packages',
        'platlib': '{platbase}/{platlibdir}/' + _LIB + '/site-packages',
        'include': '{installed_base}/include/' + _INCLUDE,
        'platinclude': '{installed_platbase}/include/' + _INCLUDE,
        'scripts': '{base}/bin',
        'data': '{base}',
        },
    'posix_home': {
        'stdlib': '{installed_base}/lib/' + _HOME,
        'platstdlib': '{base}/lib/' + _HOME,
        'purelib': '{base}/lib/' + _HOME,
        'platlib': '{base}/lib/' + _HOME,
        'include': '{installed_base}/include/' + _HOME,
        'platinclude': '{installed_base}/include/' + _HOME,
        'scripts': '{base}/bin',
        'data': '{base}',
        },
    'nt': {
        'stdlib': '{installed_base}/Lib',
        'platstdlib': '{base}/Lib',
        'purelib': '{base}/Lib/site-packages',
        'platlib': '{base}/Lib/site-packages',
        'include': '{installed_base}/Include',
        'platinclude': '{installed_base}/Include',
        'scripts': '{base}/Scripts',
        'data': '{base}',
        },
    # The "*_venv" schemes bootstrap virtual environments; distributors who
    # patch posix_prefix/nt leave them unchanged.
    'posix_venv': {
        'stdlib': '{installed_base}/{platlibdir}/' + _LIB,
        'platstdlib': '{platbase}/{platlibdir}/' + _LIB,
        'purelib': '{base}/lib/' + _LIB + '/site-packages',
        'platlib': '{platbase}/{platlibdir}/' + _LIB + '/site-packages',
        'include': '{installed_base}/include/' + _INCLUDE,
        'platinclude': '{installed_platbase}/include/' + _INCLUDE,
        'scripts': '{base}/bin',
        'data': '{base}',
        },
    'nt_venv': {
        'stdlib': '{installed_base}/Lib',
        'platstdlib': '{base}/Lib',
        'purelib': '{base}/Lib/site-packages',
        'platlib': '{base}/Lib/site-packages',
        'include': '{installed_base}/Include',
        'platinclude': '{installed_base}/Include',
        'scripts': '{base}/Scripts',
        'data': '{base}',
        },
    }

# For the OS-native venv scheme, we essentially provide an alias:
if os.name == 'nt':
    _INSTALL_SCHEMES['venv'] = _INSTALL_SCHEMES['nt_venv']
else:
    _INSTALL_SCHEMES['venv'] = _INSTALL_SCHEMES['posix_venv']


def _get_implementation():
    return 'Python'


# NOTE: site.py has copy of this function.
# Sync it when modify this function.
def _getuserbase():
    env_base = os.environ.get("PYTHONUSERBASE", None)
    if env_base:
        return env_base

    # Emscripten, iOS, tvOS, VxWorks, WASI, and watchOS have no home directories.
    if sys.version_info >= (3, 14):
        # Use _PYTHON_HOST_PLATFORM to get the correct platform when cross-compiling.
        system_name = os.environ.get('_PYTHON_HOST_PLATFORM', sys.platform).split('-')[0]
    else:
        system_name = sys.platform
    if sys.version_info >= (3, 13):
        homeless = {"emscripten", "ios", "tvos", "vxworks", "wasi", "watchos"}
    else:
        homeless = {"emscripten", "vxworks", "wasi"}
    if system_name in homeless:
        return None

    def joinuser(*args):
        return os.path.expanduser(os.path.join(*args))

    if os.name == "nt":
        base = os.environ.get("APPDATA") or "~"
        return joinuser(base, _get_implementation())

    if sys.platform == "darwin" and sys._framework:
        return joinuser("~", "Library", sys._framework,
                        f"{sys.version_info[0]}.{sys.version_info[1]}")

    return joinuser("~", ".local")

_HAS_USER_BASE = (_getuserbase() is not None)

if _HAS_USER_BASE:
    _INSTALL_SCHEMES |= {
        # NOTE: When modifying "purelib" scheme, update site._get_path() too.
        'nt_user': {
            'stdlib': '{userbase}/' + _NT_USER,
            'platstdlib': '{userbase}/' + _NT_USER,
            'purelib': '{userbase}/' + _NT_USER + '/site-packages',
            'platlib': '{userbase}/' + _NT_USER + '/site-packages',
            'include': '{userbase}/' + _NT_USER + '/Include',
            'scripts': '{userbase}/' + _NT_USER + '/Scripts',
            'data': '{userbase}',
            },
        'posix_user': {
            'stdlib': '{userbase}/{platlibdir}/' + _LIB,
            'platstdlib': '{userbase}/{platlibdir}/' + _LIB,
            'purelib': '{userbase}/lib/' + _LIB + '/site-packages',
            'platlib': '{userbase}/lib/' + _LIB + '/site-packages',
            'include': '{userbase}/include/' + _USER_INCLUDE,
            'scripts': '{userbase}/bin',
            'data': '{userbase}',
            },
        'osx_framework_user': {
            'stdlib': '{userbase}/lib/' + _HOME,
            'platstdlib': '{userbase}/lib/' + _HOME,
            'purelib': '{userbase}/lib/' + _HOME + '/site-packages',
            'platlib': '{userbase}/lib/' + _HOME + '/site-packages',
            'include': '{userbase}/include/' + _FRAMEWORK_INCLUDE,
            'scripts': '{userbase}/bin',
            'data': '{userbase}',
            },
    }

del _LIB, _INCLUDE, _HOME, _NT_USER, _USER_INCLUDE, _FRAMEWORK_INCLUDE

_SCHEME_KEYS = ('stdlib', 'platstdlib', 'purelib', 'platlib', 'include',
                'scripts', 'data')

_PY_VERSION = sys.version.split()[0]
_PY_VERSION_SHORT = f'{sys.version_info[0]}.{sys.version_info[1]}'
_PY_VERSION_SHORT_NO_DOT = f'{sys.version_info[0]}{sys.version_info[1]}'
_BASE_PREFIX = os.path.normpath(sys.base_prefix)
_BASE_EXEC_PREFIX = os.path.normpath(sys.base_exec_prefix)
# Mutex guarding initialization of _CONFIG_VARS.
_CONFIG_VARS_LOCK = threading.RLock()
_CONFIG_VARS = None
# True iff _CONFIG_VARS has been fully initialized.
_CONFIG_VARS_INITIALIZED = False
_USER_BASE = None


def _safe_realpath(path):
    try:
        return realpath(path)
    except OSError:
        return path

if sys.executable:
    _PROJECT_BASE = os.path.dirname(_safe_realpath(sys.executable))
else:
    # sys.executable can be empty if argv[0] has been changed and Python is
    # unable to retrieve the real program name
    _PROJECT_BASE = _safe_realpath(os.getcwd())

# In a virtual environment, `sys._home` gives us the target directory
# `_PROJECT_BASE` for the executable that created it when the virtual
# python is an actual executable ('venv --copies' or Windows).
_sys_home = getattr(sys, '_home', None)
if _sys_home:
    _PROJECT_BASE = _sys_home

if os.name == 'nt':
    # In a source build, the executable is in a subdirectory of the root
    # that we want (<root>\PCbuild\<platname>).
    # `_BASE_PREFIX` is used as the base installation is where the source
    # will be.  The realpath is needed to prevent mount point confusion
    # that can occur with just string comparisons.
    if _safe_realpath(_PROJECT_BASE).startswith(
            _safe_realpath(f'{_BASE_PREFIX}\\PCbuild')):
        _PROJECT_BASE = _BASE_PREFIX

# set for cross builds
if "_PYTHON_PROJECT_BASE" in os.environ:
    _PROJECT_BASE = _safe_realpath(os.environ["_PYTHON_PROJECT_BASE"])


def _is_python_build():
    for fn in ("Setup", "Setup.local"):
        if os.path.isfile(os.path.join(_PROJECT_BASE, "Modules", fn)):
            return True
    return False


if sys.version_info >= (3, 15):
    def is_python_build():
        return _is_python_build()
else:
    def is_python_build(check_home=None):
        if check_home is not None:
            import warnings
            warnings.warn(
                (
                    'The check_home argument of sysconfig.is_python_build is '
                    'deprecated and its value is ignored. '
                    'It will be removed in Python 3.15.'
                ),
                DeprecationWarning,
                stacklevel=2,
            )
        return _is_python_build()

_PYTHON_BUILD = is_python_build()

if _PYTHON_BUILD:
    for scheme in ('posix_prefix', 'posix_home'):
        # On POSIX-y platforms, Python will:
        # - Build from .h files in 'headers' (which is only added to the
        #   scheme when building CPython)
        # - Install .h files to 'include'
        scheme = _INSTALL_SCHEMES[scheme]
        scheme['headers'] = scheme['include']
        scheme['include'] = '{srcdir}/Include'
        scheme['platinclude'] = '{projectbase}/.'
    del scheme


def _subst_vars(s, local_vars):
    try:
        return s.format(**local_vars)
    except KeyError as var:
        try:
            return s.format(**os.environ)
        except KeyError:
            raise AttributeError(f'{var}') from None

def _extend_dict(target_dict, other_dict):
    target_keys = target_dict.keys()
    for key, value in other_dict.items():
        if key in target_keys:
            continue
        target_dict[key] = value


def _expand_vars(scheme, vars):
    res = {}
    if vars is None:
        vars = {}
    _extend_dict(vars, get_config_vars())
    if os.name == 'nt':
        # On Windows we want to substitute 'lib' for schemes rather
        # than the native value (without modifying vars, in case it
        # was passed in)
        vars = vars | {'platlibdir': 'lib'}

    for key, value in _INSTALL_SCHEMES[scheme].items():
        if os.name in ('posix', 'nt'):
            value = os.path.expanduser(value)
        res[key] = os.path.normpath(_subst_vars(value, vars))
    return res


def _get_preferred_schemes():
    if os.name == 'nt':
        return {
            'prefix': 'nt',
            'home': 'posix_home',
            'user': 'nt_user',
        }
    if sys.platform == 'darwin' and sys._framework:
        return {
            'prefix': 'posix_prefix',
            'home': 'posix_home',
            'user': 'osx_framework_user',
        }

    return {
        'prefix': 'posix_prefix',
        'home': 'posix_home',
        'user': 'posix_user',
    }


def get_preferred_scheme(key):
    if key == 'prefix' and sys.prefix != sys.base_prefix:
        return 'venv'
    scheme = _get_preferred_schemes()[key]
    if scheme not in _INSTALL_SCHEMES:
        raise ValueError(
            f"{key!r} returned {scheme!r}, which is not a valid scheme "
            f"on this platform"
        )
    return scheme


def get_default_scheme():
    return get_preferred_scheme('prefix')


def get_makefile_filename():
    """Return the path of the Makefile."""

    # GH-127429: When cross-compiling, use the Makefile from the target, instead of the host Python.
    if sys.version_info >= (3, 14):
        if cross_base := os.environ.get('_PYTHON_PROJECT_BASE'):
            return os.path.join(cross_base, 'Makefile')

    if _PYTHON_BUILD:
        return os.path.join(_PROJECT_BASE, "Makefile")

    if hasattr(sys, 'abiflags'):
        config_dir_name = f'config-{_PY_VERSION_SHORT}{sys.abiflags}'
    else:
        config_dir_name = 'config'

    if hasattr(sys.implementation, '_multiarch'):
        config_dir_name += f'-{sys.implementation._multiarch}'

    return os.path.join(get_path('stdlib'), config_dir_name, 'Makefile')


def _get_sysconfigdata_name():
    multiarch = getattr(sys.implementation, '_multiarch', '')
    return os.environ.get(
        '_PYTHON_SYSCONFIGDATA_NAME',
        f'_sysconfigdata_{sys.abiflags}_{sys.platform}_{multiarch}',
    )


def _get_sysconfigdata():
    """Return the build-time variables of the running Molt binary.

    CPython imports them from the _sysconfigdata module its build generated
    from the Makefile and pyconfig.h. A Molt binary has neither: each value
    here is derived from the runtime authority that owns the fact, as PyPy's
    _sysconfigdata derives its values from the running interpreter. This is
    the one derivation; _sysconfig.config_vars() reads its keys from here.
    """
    # 'd' marks a debug ABI and 't' a free-threaded one, as CPython's
    # configure.ac spells ABIFLAGS.
    abiflags = _MOLT_SYS_ABIFLAGS()
    data = {
        'ABIFLAGS': abiflags,
        'MACHDEP': sys.platform,
        'Py_DEBUG': int('d' in abiflags),
        'VERSION': _PY_VERSION_SHORT,
    }
    if sys.version_info >= (3, 13):
        data['Py_GIL_DISABLED'] = int('t' in abiflags)
    # GH-99201: _imp.extension_suffixes() is empty when the interpreter
    # cannot load extension modules at run time; then EXT_SUFFIX is unset.
    import _imp
    suffixes = _imp.extension_suffixes()
    if suffixes:
        data['EXT_SUFFIX'] = suffixes[0]
    return data


def _init_posix(vars):
    """Initialize the module as appropriate for POSIX systems."""
    # GH-126920: Make sure we don't overwrite any of the keys already set
    vars.update(_get_sysconfigdata() | vars)


def _init_non_posix(vars):
    """Initialize the module as appropriate for NT"""
    # set basic install directories
    vars['LIBDEST'] = get_path('stdlib')
    vars['BINLIBDEST'] = get_path('platstdlib')
    vars['INCLUDEPY'] = get_path('include')

    if sys.version_info >= (3, 13):
        import _sysconfig
        # Add EXT_SUFFIX, SOABI, Py_DEBUG, and Py_GIL_DISABLED
        vars.update(_sysconfig.config_vars())
        if sys.version_info >= (3, 14):
            # NOTE: ABIFLAGS is only an emulated value. It is not present during build
            #       on Windows. sys.abiflags is absent on Windows and vars['abiflags']
            #       is already widely used to calculate paths, so it should remain an
            #       empty string.
            vars['ABIFLAGS'] = ''.join(
                (
                    't' if vars['Py_GIL_DISABLED'] else '',
                    '_d' if vars['Py_DEBUG'] else '',
                ),
            )
        vars['LIBDIR'] = _safe_realpath(os.path.join(get_config_var('installed_base'), 'libs'))
        # CPython also names its DLL through sys.dllhandle; a Molt binary
        # embeds its runtime and has no sys.dllhandle.
    else:
        import _imp
        try:
            # GH-99201: _imp.extension_suffixes may be empty when
            # HAVE_DYNAMIC_LOADING is not set. In this case, don't set EXT_SUFFIX.
            vars['EXT_SUFFIX'] = _imp.extension_suffixes()[0]
        except IndexError:
            pass
    vars['EXE'] = '.exe'
    vars['VERSION'] = _PY_VERSION_SHORT_NO_DOT
    vars['BINDIR'] = os.path.dirname(_safe_realpath(sys.executable))
    if sys.version_info >= (3, 15):
        # No standard path exists on Windows for this, but we'll check
        # whether someone is imitating a POSIX-like layout
        check_tzpath = os.path.join(vars['prefix'], 'share', 'zoneinfo')
        if os.path.exists(check_tzpath):
            vars['TZPATH'] = check_tzpath
        else:
            vars['TZPATH'] = ''
    else:
        vars['TZPATH'] = ''

#
# public APIs
#


def parse_config_h(fp, vars=None):
    """Parse a config.h-style file.

    A dictionary containing name/value pairs is returned.  If an
    optional dictionary is passed in as the second argument, it is
    used instead of a new dictionary.
    """
    if vars is None:
        vars = {}
    import re
    if sys.version_info >= (3, 14):
        name_rx = '(?:[A-Z]|_Py_)[A-Za-z0-9_]+'
    else:
        name_rx = '[A-Z][A-Za-z0-9_]+'
    define_rx = re.compile(fr"#define ({name_rx}) (.*)\n")
    undef_rx = re.compile(fr"/[*] #undef ({name_rx}) [*]/\n")
    quoted_rx = re.compile(r'"(.*)"') if sys.version_info >= (3, 15) else None

    while True:
        line = fp.readline()
        if not line:
            break
        m = define_rx.match(line)
        if m:
            n, v = m.group(1, 2)
            try:
                if n in _ALWAYS_STR:
                    raise ValueError
                v = int(v)
            except ValueError:
                if quoted_rx is not None and (mq := quoted_rx.fullmatch(v)):
                    v = mq.group(1)
            vars[n] = v
        else:
            m = undef_rx.match(line)
            if m:
                vars[m.group(1)] = 0
    return vars


def get_config_h_filename():
    """Return the path of pyconfig.h."""
    if _PYTHON_BUILD:
        if os.name == "nt":
            inc_dir = os.path.join(_PROJECT_BASE, "PC")
        else:
            inc_dir = _PROJECT_BASE
    else:
        inc_dir = get_path('platinclude')
    return os.path.join(inc_dir, 'pyconfig.h')


def get_scheme_names():
    """Return a tuple containing the schemes names."""
    return tuple(sorted(_INSTALL_SCHEMES))


def get_path_names():
    """Return a tuple containing the paths names."""
    return _SCHEME_KEYS


def get_paths(scheme=get_default_scheme(), vars=None, expand=True):
    """Return a mapping containing an install scheme.

    ``scheme`` is the install scheme name. If not provided, it will
    return the default scheme for the current platform.
    """
    if expand:
        return _expand_vars(scheme, vars)
    else:
        return _INSTALL_SCHEMES[scheme]


def get_path(name, scheme=get_default_scheme(), vars=None, expand=True):
    """Return a path corresponding to the scheme.

    ``scheme`` is the install scheme name.
    """
    return get_paths(scheme, vars, expand)[name]


def _init_config_vars():
    global _CONFIG_VARS
    _CONFIG_VARS = {}

    prefix = os.path.normpath(sys.prefix)
    exec_prefix = os.path.normpath(sys.exec_prefix)

    try:
        abiflags = sys.abiflags
    except AttributeError:
        # sys.abiflags may not be defined on all platforms.
        abiflags = ''

    # Python 3.14 reads the build variables first, so the computed values
    # below take precedence; earlier versions read them after.
    if os.name == 'posix' and sys.version_info >= (3, 14):
        _init_posix(_CONFIG_VARS)

    # Normalized versions of prefix and exec_prefix are handy to have;
    # in fact, these are the standard versions used most places in the
    # Distutils.
    _CONFIG_VARS['prefix'] = prefix
    _CONFIG_VARS['exec_prefix'] = exec_prefix
    _CONFIG_VARS['py_version'] = _PY_VERSION
    _CONFIG_VARS['py_version_short'] = _PY_VERSION_SHORT
    _CONFIG_VARS['py_version_nodot'] = _PY_VERSION_SHORT_NO_DOT
    _CONFIG_VARS['installed_base'] = _BASE_PREFIX
    _CONFIG_VARS['base'] = prefix
    _CONFIG_VARS['installed_platbase'] = _BASE_EXEC_PREFIX
    _CONFIG_VARS['platbase'] = exec_prefix
    _CONFIG_VARS['projectbase'] = _PROJECT_BASE
    _CONFIG_VARS['platlibdir'] = sys.platlibdir
    if sys.version_info >= (3, 13):
        _CONFIG_VARS['implementation'] = _get_implementation()
        _CONFIG_VARS['implementation_lower'] = _get_implementation().lower()
    _CONFIG_VARS['abiflags'] = abiflags
    try:
        _CONFIG_VARS['py_version_nodot_plat'] = sys.winver.replace('.', '')
    except AttributeError:
        _CONFIG_VARS['py_version_nodot_plat'] = ''

    if os.name == 'nt':
        _init_non_posix(_CONFIG_VARS)
        # CPython names its build tree's VPATH here. A Molt binary was not
        # built in a CPython tree, so it has no VPATH.
    if os.name == 'posix' and sys.version_info < (3, 14):
        _init_posix(_CONFIG_VARS)
    if _HAS_USER_BASE:
        # Setting 'userbase' is done below the call to the
        # init function to enable using 'get_config_var' in
        # the init-function.
        _CONFIG_VARS['userbase'] = _getuserbase()

    if sys.version_info >= (3, 13):
        # e.g., 't' for free-threaded or '' for default build
        _CONFIG_VARS['abi_thread'] = 't' if _CONFIG_VARS.get('Py_GIL_DISABLED') else ''

    # Always convert srcdir to an absolute path
    srcdir = _CONFIG_VARS.get('srcdir', _PROJECT_BASE)
    if os.name == 'posix':
        if _PYTHON_BUILD:
            # If srcdir is a relative path (typically '.' or '..')
            # then it should be interpreted relative to the directory
            # containing Makefile.
            base = os.path.dirname(get_makefile_filename())
            srcdir = os.path.join(base, srcdir)
        else:
            # srcdir is not meaningful since the installation is
            # spread about the filesystem.  We choose the
            # directory containing the Makefile since we know it
            # exists.
            srcdir = os.path.dirname(get_makefile_filename())
    _CONFIG_VARS['srcdir'] = _safe_realpath(srcdir)

    # OS X platforms require special customization to handle
    # multi-architecture, multi-os-version installers
    if sys.platform == 'darwin':
        import _osx_support
        _osx_support.customize_config_vars(_CONFIG_VARS)

    global _CONFIG_VARS_INITIALIZED
    _CONFIG_VARS_INITIALIZED = True


def get_config_vars(*args):
    """With no arguments, return a dictionary of all configuration
    variables relevant for the current platform.

    On Unix, this means every variable defined in Python's installed Makefile;
    On Windows it's a much smaller set.

    With arguments, return a list of values that result from looking up
    each argument in the configuration variable dictionary.
    """
    global _CONFIG_VARS_INITIALIZED

    # Avoid claiming the lock once initialization is complete.
    if _CONFIG_VARS_INITIALIZED:
        # GH-126789: If sys.prefix (and, from 3.14, sys.exec_prefix) were
        # updated, invalidate the cache.
        prefix = os.path.normpath(sys.prefix)
        exec_prefix = os.path.normpath(sys.exec_prefix)
        if sys.version_info >= (3, 14):
            stale = (_CONFIG_VARS['prefix'] != prefix
                     or _CONFIG_VARS['exec_prefix'] != exec_prefix)
        else:
            stale = _CONFIG_VARS['base'] != prefix
        if stale:
            with _CONFIG_VARS_LOCK:
                _CONFIG_VARS_INITIALIZED = False
                _init_config_vars()
    else:
        # Initialize the config_vars cache.
        with _CONFIG_VARS_LOCK:
            # Test again with the lock held to avoid races. Note that
            # we test _CONFIG_VARS here, not _CONFIG_VARS_INITIALIZED,
            # to ensure that recursive calls to get_config_vars()
            # don't re-enter init_config_vars().
            if _CONFIG_VARS is None:
                _init_config_vars()

    if args:
        vals = []
        for name in args:
            vals.append(_CONFIG_VARS.get(name))
        return vals
    else:
        return _CONFIG_VARS


def get_config_var(name):
    """Return the value of a single variable using the dictionary returned by
    'get_config_vars()'.

    Equivalent to get_config_vars().get(name)
    """
    return get_config_vars().get(name)


def _windows_platform():
    """The Windows platform tag of the running binary.

    CPython 3.12-3.14 read the architecture from the compiler tag in
    sys.version, and 3.15 asks _sysconfig.get_platform(), which reads the
    build's compiler macros. A Molt binary's sys.version names no compiler;
    its runtime names its target architecture once (platform.processor()).
    """
    machine = _MOLT_TARGET_MACHINE()
    if machine == 'x86_64':
        return 'win-amd64'
    if machine == 'aarch64':
        return 'win-arm64'
    return sys.platform


def get_platform():
    """Return a string that identifies the current platform.

    This is used mainly to distinguish platform-specific build directories and
    platform-specific built distributions.  Typically includes the OS name and
    version and the architecture (as supplied by 'os.uname()'), although the
    exact information included depends on the OS; on Linux, the kernel version
    isn't particularly important.

    Examples of returned values:


    Windows:

    - win-amd64 (64-bit Windows on AMD64, aka x86_64, Intel64, and EM64T)
    - win-arm64 (64-bit Windows on ARM64, aka AArch64)
    - win32 (all others - specifically, sys.platform is returned)

    POSIX based OS:

    - linux-x86_64
    - macosx-15.5-arm64
    - macosx-26.0-universal2 (macOS on Apple Silicon or Intel)
    - android-24-arm64_v8a

    For other non-POSIX platforms, currently just returns :data:`sys.platform`."""
    if os.name == 'nt':
        return _windows_platform()

    if os.name != "posix" or not hasattr(os, 'uname'):
        # XXX what about the architecture? NT is Intel or Alpha
        return sys.platform

    # Set for cross builds explicitly
    if "_PYTHON_HOST_PLATFORM" in os.environ:
        if sys.version_info < (3, 14):
            return os.environ["_PYTHON_HOST_PLATFORM"]
        osname, _, machine = os.environ["_PYTHON_HOST_PLATFORM"].partition('-')
        release = None
    else:
        # Try to distinguish various flavours of Unix
        osname, host, release, version, machine = os.uname()

        # Convert the OS name to lowercase, remove '/' characters, and translate
        # spaces (for "Power Macintosh")
        osname = osname.lower().replace('/', '')
        machine = machine.replace(' ', '_')
        machine = machine.replace('/', '-')

    if sys.version_info >= (3, 13) and (
        sys.platform == "android"
        or (sys.version_info >= (3, 14) and osname == "android")
    ):
        osname = "android"
        release = get_config_var("ANDROID_API_LEVEL")

        # Wheel tags use the ABI names from Android's own tools.
        # When Python is running on 32-bit ARM Android on a 64-bit ARM kernel,
        # 'os.uname().machine' is 'armv8l'. Such devices run the same userspace
        # code as 'armv7l' devices.
        # During the build process of the Android testbed when targeting 32-bit ARM,
        # '_PYTHON_HOST_PLATFORM' is 'arm-linux-androideabi', so 'machine' becomes
        # 'arm'.
        machine = {
            "aarch64": "arm64_v8a",
            "arm": "armeabi_v7a",
            "armv7l": "armeabi_v7a",
            "armv8l": "armeabi_v7a",
            "i686": "x86",
            "x86_64": "x86_64",
        }[machine]
    elif osname[:5] == "linux":
        # At least on Linux/Intel, 'machine' is the processor --
        # i386, etc.
        # XXX what about Alpha, SPARC, etc?
        return  f"{osname}-{machine}"
    elif osname[:5] == "sunos":
        if release[0] >= "5":           # SunOS 5 == Solaris 2
            osname = "solaris"
            release = f"{int(release[0]) - 3}.{release[2:]}"
            # We can't use "platform.architecture()[0]" because a
            # bootstrap problem. We use a dict to get an error
            # if some suspicious happens.
            bitness = {2147483647:"32bit", 9223372036854775807:"64bit"}
            machine += f".{bitness[sys.maxsize]}"
        # fall through to standard osname-release-machine representation
    elif osname[:3] == "aix":
        from _aix_support import aix_platform
        return aix_platform()
    elif osname[:6] == "cygwin":
        osname = "cygwin"
        import re
        rel_re = re.compile(r'[\d.]+')
        m = rel_re.match(release)
        if m:
            release = m.group()
    elif osname[:6] == "darwin":
        if sys.version_info >= (3, 13) and sys.platform == "ios":
            release = get_config_vars().get("IPHONEOS_DEPLOYMENT_TARGET", "13.0")
            osname = sys.platform
            machine = sys.implementation._multiarch
        else:
            import _osx_support
            osname, release, machine = _osx_support.get_platform_osx(
                                                get_config_vars(),
                                                osname, release, machine)

    if sys.version_info >= (3, 14):
        return '-'.join(map(str, filter(None, (osname, release, machine))))
    return f"{osname}-{release}-{machine}"


def get_python_version():
    return _PY_VERSION_SHORT


def _get_python_version_abi():
    return _PY_VERSION_SHORT + get_config_var("abi_thread")


def expand_makefile_vars(s, vars):
    """Expand Makefile-style variables -- "${foo}" or "$(foo)" -- in
    'string' according to 'vars' (a dictionary mapping variable names to
    values).  Variables not present in 'vars' are silently expanded to the
    empty string.  The variable values in 'vars' should not contain further
    variable expansions; if 'vars' is the output of 'parse_makefile()',
    you're fine.  Returns a variable-expanded version of 's'.
    """

    if sys.version_info >= (3, 14):
        import warnings
        warnings.warn(
            'sysconfig.expand_makefile_vars is deprecated and will be removed in '
            'Python 3.16. Use sysconfig.get_paths(vars=...) instead.',
            DeprecationWarning,
            stacklevel=2,
        )

    import re

    _findvar1_rx = r"\$\(([A-Za-z][A-Za-z0-9_]*)\)"
    _findvar2_rx = r"\${([A-Za-z][A-Za-z0-9_]*)}"

    # This algorithm does multiple expansion, so if vars['foo'] contains
    # "${bar}", it will expand ${foo} to ${bar}, and then expand
    # ${bar}... and so forth.  This is fine as long as 'vars' comes from
    # 'parse_makefile()', which takes care of such expansions eagerly,
    # according to make's variable expansion semantics.

    while True:
        m = re.search(_findvar1_rx, s) or re.search(_findvar2_rx, s)
        if m:
            (beg, end) = m.span()
            s = s[0:beg] + vars.get(m.group(1)) + s[end:]
        else:
            break
    return s


globals().pop("_require_intrinsic", None)
