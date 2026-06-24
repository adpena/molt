from __future__ import annotations

import contextlib
import inspect
import pkgutil
import sys
from functools import partial, wraps
from types import ModuleType


_context = sys.modules[globals()["_MOLT_CONTEXT_MODULE"]]

AsyncMock = _context.AsyncMock
DEFAULT = _context.DEFAULT
InvalidSpecError = _context.InvalidSpecError
MagicMock = _context.MagicMock
NonCallableMagicMock = _context.NonCallableMagicMock
NonCallableMock = _context.NonCallableMock
_builtins = _context._builtins
_check_spec_arg_typos = _context._check_spec_arg_typos
create_autospec = _context.create_autospec
_instance_callable = _context._instance_callable
_is_async_obj = _context._is_async_obj
_is_instance_mock = _context._is_instance_mock
_is_list = _context._is_list


class _patch(object):
    attribute_name = None
    _active_patches = []

    def __init__(
        self,
        getter,
        attribute,
        new,
        spec,
        create,
        spec_set,
        autospec,
        new_callable,
        kwargs,
        *,
        unsafe=False,
    ):
        if new_callable is not None:
            if new is not DEFAULT:
                raise ValueError("Cannot use 'new' and 'new_callable' together")
            if autospec is not None:
                raise ValueError("Cannot use 'autospec' and 'new_callable' together")
        if not unsafe:
            _check_spec_arg_typos(kwargs)
        if _is_instance_mock(spec):
            raise InvalidSpecError(
                f"Cannot spec attr {attribute!r} as the spec "
                f"has already been mocked out. [spec={spec!r}]"
            )
        if _is_instance_mock(spec_set):
            raise InvalidSpecError(
                f"Cannot spec attr {attribute!r} as the spec_set "
                f"target has already been mocked out. [spec_set={spec_set!r}]"
            )

        self.getter = getter
        self.attribute = attribute
        self.new = new
        self.new_callable = new_callable
        self.spec = spec
        self.create = create
        self.has_local = False
        self.spec_set = spec_set
        self.autospec = autospec
        self.kwargs = kwargs
        self.additional_patchers = []
        self.is_started = False

    def copy(self):
        patcher = _patch(
            self.getter,
            self.attribute,
            self.new,
            self.spec,
            self.create,
            self.spec_set,
            self.autospec,
            self.new_callable,
            self.kwargs,
        )
        patcher.attribute_name = self.attribute_name
        patcher.additional_patchers = [p.copy() for p in self.additional_patchers]
        return patcher

    def __call__(self, func):
        if isinstance(func, type):
            return self.decorate_class(func)
        if inspect.iscoroutinefunction(func):
            return self.decorate_async_callable(func)
        return self.decorate_callable(func)

    def decorate_class(self, klass):
        for attr in dir(klass):
            if not attr.startswith(patch.TEST_PREFIX):
                continue

            attr_value = getattr(klass, attr)
            if not hasattr(attr_value, "__call__"):
                continue

            patcher = self.copy()
            setattr(klass, attr, patcher(attr_value))
        return klass

    @contextlib.contextmanager
    def decoration_helper(self, patched, args, keywargs):
        extra_args = []
        with contextlib.ExitStack() as exit_stack:
            for patching in patched.patchings:
                arg = exit_stack.enter_context(patching)
                if patching.attribute_name is not None:
                    keywargs.update(arg)
                elif patching.new is DEFAULT:
                    extra_args.append(arg)

            args += tuple(extra_args)
            yield (args, keywargs)

    def decorate_callable(self, func):
        # NB. Keep the method in sync with decorate_async_callable()
        if hasattr(func, "patchings"):
            func.patchings.append(self)
            return func

        @wraps(func)
        def patched(*args, **keywargs):
            with self.decoration_helper(patched, args, keywargs) as (
                newargs,
                newkeywargs,
            ):
                return func(*newargs, **newkeywargs)

        patched.patchings = [self]
        return patched

    def decorate_async_callable(self, func):
        # NB. Keep the method in sync with decorate_callable()
        if hasattr(func, "patchings"):
            func.patchings.append(self)
            return func

        @wraps(func)
        async def patched(*args, **keywargs):
            with self.decoration_helper(patched, args, keywargs) as (
                newargs,
                newkeywargs,
            ):
                return await func(*newargs, **newkeywargs)

        patched.patchings = [self]
        return patched

    def get_original(self):
        target = self.getter()
        name = self.attribute

        original = DEFAULT
        local = False

        try:
            original = target.__dict__[name]
        except (AttributeError, KeyError):
            original = getattr(target, name, DEFAULT)
        else:
            local = True

        if name in _builtins and isinstance(target, ModuleType):
            self.create = True

        if not self.create and original is DEFAULT:
            raise AttributeError("%s does not have the attribute %r" % (target, name))
        return original, local

    def __enter__(self):
        """Perform the patch."""
        if self.is_started:
            raise RuntimeError("Patch is already started")

        new, spec, spec_set = self.new, self.spec, self.spec_set
        autospec, kwargs = self.autospec, self.kwargs
        new_callable = self.new_callable
        self.target = self.getter()

        # normalise False to None
        if spec is False:
            spec = None
        if spec_set is False:
            spec_set = None
        if autospec is False:
            autospec = None

        if spec is not None and autospec is not None:
            raise TypeError("Can't specify spec and autospec")
        if (spec is not None or autospec is not None) and spec_set not in (True, None):
            raise TypeError("Can't provide explicit spec_set *and* spec or autospec")

        original, local = self.get_original()

        if new is DEFAULT and autospec is None:
            inherit = False
            if spec is True:
                # set spec to the object we are replacing
                spec = original
                if spec_set is True:
                    spec_set = original
                    spec = None
            elif spec is not None:
                if spec_set is True:
                    spec_set = spec
                    spec = None
            elif spec_set is True:
                spec_set = original

            if spec is not None or spec_set is not None:
                if original is DEFAULT:
                    raise TypeError("Can't use 'spec' with create=True")
                if isinstance(original, type):
                    # If we're patching out a class and there is a spec
                    inherit = True

            # Determine the Klass to use
            if new_callable is not None:
                Klass = new_callable
            elif spec is None and _is_async_obj(original):
                Klass = AsyncMock
            elif spec is not None or spec_set is not None:
                this_spec = spec
                if spec_set is not None:
                    this_spec = spec_set
                if _is_list(this_spec):
                    not_callable = "__call__" not in this_spec
                else:
                    not_callable = not callable(this_spec)
                if _is_async_obj(this_spec):
                    Klass = AsyncMock
                elif not_callable:
                    Klass = NonCallableMagicMock
                else:
                    Klass = MagicMock
            else:
                Klass = MagicMock

            _kwargs = {}
            if spec is not None:
                _kwargs["spec"] = spec
            if spec_set is not None:
                _kwargs["spec_set"] = spec_set

            # add a name to mocks
            if (
                isinstance(Klass, type)
                and issubclass(Klass, NonCallableMock)
                and self.attribute
            ):
                _kwargs["name"] = self.attribute

            _kwargs.update(kwargs)
            new = Klass(**_kwargs)

            if inherit and _is_instance_mock(new):
                # we can only tell if the instance should be callable if the
                # spec is not a list
                this_spec = spec
                if spec_set is not None:
                    this_spec = spec_set
                if not _is_list(this_spec) and not _instance_callable(this_spec):
                    Klass = NonCallableMagicMock

                _kwargs.pop("name")
                new.return_value = Klass(_new_parent=new, _new_name="()", **_kwargs)
        elif autospec is not None:
            # spec is ignored, new *must* be default, spec_set is treated
            # as a boolean. Should we check spec is not None and that spec_set
            # is a bool?
            if new is not DEFAULT:
                raise TypeError(
                    "autospec creates the mock for you. Can't specify autospec and new."
                )
            if original is DEFAULT:
                raise TypeError("Can't use 'autospec' with create=True")
            spec_set = bool(spec_set)
            if autospec is True:
                autospec = original

            if _is_instance_mock(self.target):
                raise InvalidSpecError(
                    f"Cannot autospec attr {self.attribute!r} as the patch "
                    f"target has already been mocked out. "
                    f"[target={self.target!r}, attr={autospec!r}]"
                )
            if _is_instance_mock(autospec):
                target_name = getattr(self.target, "__name__", self.target)
                raise InvalidSpecError(
                    f"Cannot autospec attr {self.attribute!r} from target "
                    f"{target_name!r} as it has already been mocked out. "
                    f"[target={self.target!r}, attr={autospec!r}]"
                )

            new = create_autospec(
                autospec, spec_set=spec_set, _name=self.attribute, **kwargs
            )
        elif kwargs:
            # can't set keyword args when we aren't creating the mock
            # XXXX If new is a Mock we could call new.configure_mock(**kwargs)
            raise TypeError("Can't pass kwargs to a mock we aren't creating")

        new_attr = new

        self.temp_original = original
        self.is_local = local
        self._exit_stack = contextlib.ExitStack()
        self.is_started = True
        try:
            setattr(self.target, self.attribute, new_attr)
            if self.attribute_name is not None:
                extra_args = {}
                if self.new is DEFAULT:
                    extra_args[self.attribute_name] = new
                for patching in self.additional_patchers:
                    arg = self._exit_stack.enter_context(patching)
                    if patching.new is DEFAULT:
                        extra_args.update(arg)
                return extra_args

            return new
        except BaseException:
            if not self.__exit__(*sys.exc_info()):
                raise

    def __exit__(self, *exc_info):
        """Undo the patch."""
        if not self.is_started:
            return

        if self.is_local and self.temp_original is not DEFAULT:
            setattr(self.target, self.attribute, self.temp_original)
        else:
            delattr(self.target, self.attribute)
            if not self.create and (
                not hasattr(self.target, self.attribute)
                or self.attribute
                in (
                    "__doc__",
                    "__module__",
                    "__defaults__",
                    "__annotations__",
                    "__kwdefaults__",
                )
            ):
                # needed for proxy objects like django settings
                setattr(self.target, self.attribute, self.temp_original)

        del self.temp_original
        del self.is_local
        del self.target
        exit_stack = self._exit_stack
        del self._exit_stack
        self.is_started = False
        return exit_stack.__exit__(*exc_info)

    def start(self):
        """Activate a patch, returning any created mock."""
        result = self.__enter__()
        self._active_patches.append(self)
        return result

    def stop(self):
        """Stop an active patch."""
        try:
            self._active_patches.remove(self)
        except ValueError:
            # If the patch hasn't been started this will fail
            return None

        return self.__exit__(None, None, None)


def _get_target(target):
    try:
        target, attribute = target.rsplit(".", 1)
    except (TypeError, ValueError, AttributeError):
        raise TypeError(f"Need a valid target to patch. You supplied: {target!r}")
    return partial(pkgutil.resolve_name, target), attribute


def _patch_object(
    target,
    attribute,
    new=DEFAULT,
    spec=None,
    create=False,
    spec_set=None,
    autospec=None,
    new_callable=None,
    *,
    unsafe=False,
    **kwargs,
):
    """
    patch the named member (`attribute`) on an object (`target`) with a mock
    object.

    `patch.object` can be used as a decorator, class decorator or a context
    manager. Arguments `new`, `spec`, `create`, `spec_set`,
    `autospec` and `new_callable` have the same meaning as for `patch`. Like
    `patch`, `patch.object` takes arbitrary keyword arguments for configuring
    the mock object it creates.

    When used as a class decorator `patch.object` honours `patch.TEST_PREFIX`
    for choosing which methods to wrap.
    """
    if type(target) is str:
        raise TypeError(
            f"{target!r} must be the actual object to be patched, not a str"
        )
    def getter():
        return target

    return _patch(
        getter,
        attribute,
        new,
        spec,
        create,
        spec_set,
        autospec,
        new_callable,
        kwargs,
        unsafe=unsafe,
    )


def _patch_multiple(
    target,
    spec=None,
    create=False,
    spec_set=None,
    autospec=None,
    new_callable=None,
    **kwargs,
):
    """Perform multiple patches in a single call. It takes the object to be
    patched (either as an object or a string to fetch the object by importing)
    and keyword arguments for the patches::

        with patch.multiple(settings, FIRST_PATCH='one', SECOND_PATCH='two'):
            ...

    Use `DEFAULT` as the value if you want `patch.multiple` to create
    mocks for you. In this case the created mocks are passed into a decorated
    function by keyword, and a dictionary is returned when `patch.multiple` is
    used as a context manager.

    `patch.multiple` can be used as a decorator, class decorator or a context
    manager. The arguments `spec`, `spec_set`, `create`,
    `autospec` and `new_callable` have the same meaning as for `patch`. These
    arguments will be applied to *all* patches done by `patch.multiple`.

    When used as a class decorator `patch.multiple` honours `patch.TEST_PREFIX`
    for choosing which methods to wrap.
    """
    if type(target) is str:
        getter = partial(pkgutil.resolve_name, target)
    else:
        def getter():
            return target

    if not kwargs:
        raise ValueError(
            "Must supply at least one keyword argument with patch.multiple"
        )
    # need to wrap in a list for python 3, where items is a view
    items = list(kwargs.items())
    attribute, new = items[0]
    patcher = _patch(
        getter, attribute, new, spec, create, spec_set, autospec, new_callable, {}
    )
    patcher.attribute_name = attribute
    for attribute, new in items[1:]:
        this_patcher = _patch(
            getter, attribute, new, spec, create, spec_set, autospec, new_callable, {}
        )
        this_patcher.attribute_name = attribute
        patcher.additional_patchers.append(this_patcher)
    return patcher


def patch(
    target,
    new=DEFAULT,
    spec=None,
    create=False,
    spec_set=None,
    autospec=None,
    new_callable=None,
    *,
    unsafe=False,
    **kwargs,
):
    """
    `patch` acts as a function decorator, class decorator or a context
    manager. Inside the body of the function or with statement, the `target`
    is patched with a `new` object. When the function/with statement exits
    the patch is undone.

    If `new` is omitted, then the target is replaced with an
    `AsyncMock if the patched object is an async function or a
    `MagicMock` otherwise. If `patch` is used as a decorator and `new` is
    omitted, the created mock is passed in as an extra argument to the
    decorated function. If `patch` is used as a context manager the created
    mock is returned by the context manager.

    `target` should be a string in the form `'package.module.ClassName'`. The
    `target` is imported and the specified object replaced with the `new`
    object, so the `target` must be importable from the environment you are
    calling `patch` from. The target is imported when the decorated function
    is executed, not at decoration time.

    The `spec` and `spec_set` keyword arguments are passed to the `MagicMock`
    if patch is creating one for you.

    In addition you can pass `spec=True` or `spec_set=True`, which causes
    patch to pass in the object being mocked as the spec/spec_set object.

    `new_callable` allows you to specify a different class, or callable object,
    that will be called to create the `new` object. By default `AsyncMock` is
    used for async functions and `MagicMock` for the rest.

    A more powerful form of `spec` is `autospec`. If you set `autospec=True`
    then the mock will be created with a spec from the object being replaced.
    All attributes of the mock will also have the spec of the corresponding
    attribute of the object being replaced. Methods and functions being
    mocked will have their arguments checked and will raise a `TypeError` if
    they are called with the wrong signature. For mocks replacing a class,
    their return value (the 'instance') will have the same spec as the class.

    Instead of `autospec=True` you can pass `autospec=some_object` to use an
    arbitrary object as the spec instead of the one being replaced.

    By default `patch` will fail to replace attributes that don't exist. If
    you pass in `create=True`, and the attribute doesn't exist, patch will
    create the attribute for you when the patched function is called, and
    delete it again afterwards. This is useful for writing tests against
    attributes that your production code creates at runtime. It is off by
    default because it can be dangerous. With it switched on you can write
    passing tests against APIs that don't actually exist!

    Patch can be used as a `TestCase` class decorator. It works by
    decorating each test method in the class. This reduces the boilerplate
    code when your test methods share a common patchings set. `patch` finds
    tests by looking for method names that start with `patch.TEST_PREFIX`.
    By default this is `test`, which matches the way `unittest` finds tests.
    You can specify an alternative prefix by setting `patch.TEST_PREFIX`.

    Patch can be used as a context manager, with the with statement. Here the
    patching applies to the indented block after the with statement. If you
    use "as" then the patched object will be bound to the name after the
    "as"; very useful if `patch` is creating a mock object for you.

    Patch will raise a `RuntimeError` if passed some common misspellings of
    the arguments autospec and spec_set. Pass the argument `unsafe` with the
    value True to disable that check.

    `patch` takes arbitrary keyword arguments. These will be passed to
    `AsyncMock` if the patched object is asynchronous, to `MagicMock`
    otherwise or to `new_callable` if specified.

    `patch.dict(...)`, `patch.multiple(...)` and `patch.object(...)` are
    available for alternate use-cases.
    """
    getter, attribute = _get_target(target)
    return _patch(
        getter,
        attribute,
        new,
        spec,
        create,
        spec_set,
        autospec,
        new_callable,
        kwargs,
        unsafe=unsafe,
    )


class _patch_dict(object):
    """
    Patch a dictionary, or dictionary like object, and restore the dictionary
    to its original state after the test.

    `in_dict` can be a dictionary or a mapping like container. If it is a
    mapping then it must at least support getting, setting and deleting items
    plus iterating over keys.

    `in_dict` can also be a string specifying the name of the dictionary, which
    will then be fetched by importing it.

    `values` can be a dictionary of values to set in the dictionary. `values`
    can also be an iterable of `(key, value)` pairs.

    If `clear` is True then the dictionary will be cleared before the new
    values are set.

    `patch.dict` can also be called with arbitrary keyword arguments to set
    values in the dictionary::

        with patch.dict('sys.modules', mymodule=Mock(), other_module=Mock()):
            ...

    `patch.dict` can be used as a context manager, decorator or class
    decorator. When used as a class decorator `patch.dict` honours
    `patch.TEST_PREFIX` for choosing which methods to wrap.
    """

    def __init__(self, in_dict, values=(), clear=False, **kwargs):
        self.in_dict = in_dict
        # support any argument supported by dict(...) constructor
        self.values = dict(values)
        self.values.update(kwargs)
        self.clear = clear
        self._original = None

    def __call__(self, f):
        if isinstance(f, type):
            return self.decorate_class(f)
        if inspect.iscoroutinefunction(f):
            return self.decorate_async_callable(f)
        return self.decorate_callable(f)

    def decorate_callable(self, f):
        @wraps(f)
        def _inner(*args, **kw):
            self._patch_dict()
            try:
                return f(*args, **kw)
            finally:
                self._unpatch_dict()

        return _inner

    def decorate_async_callable(self, f):
        @wraps(f)
        async def _inner(*args, **kw):
            self._patch_dict()
            try:
                return await f(*args, **kw)
            finally:
                self._unpatch_dict()

        return _inner

    def decorate_class(self, klass):
        for attr in dir(klass):
            attr_value = getattr(klass, attr)
            if attr.startswith(patch.TEST_PREFIX) and hasattr(attr_value, "__call__"):
                decorator = _patch_dict(self.in_dict, self.values, self.clear)
                decorated = decorator(attr_value)
                setattr(klass, attr, decorated)
        return klass

    def __enter__(self):
        """Patch the dict."""
        self._patch_dict()
        return self.in_dict

    def _patch_dict(self):
        values = self.values
        if isinstance(self.in_dict, str):
            self.in_dict = pkgutil.resolve_name(self.in_dict)
        in_dict = self.in_dict
        clear = self.clear

        try:
            original = in_dict.copy()
        except AttributeError:
            # dict like object with no copy method
            # must support iteration over keys
            original = {}
            for key in in_dict:
                original[key] = in_dict[key]
        self._original = original

        if clear:
            _clear_dict(in_dict)

        try:
            in_dict.update(values)
        except AttributeError:
            # dict like object with no update method
            for key in values:
                in_dict[key] = values[key]

    def _unpatch_dict(self):
        in_dict = self.in_dict
        original = self._original

        _clear_dict(in_dict)

        try:
            in_dict.update(original)
        except AttributeError:
            for key in original:
                in_dict[key] = original[key]

    def __exit__(self, *args):
        """Unpatch the dict."""
        if self._original is not None:
            self._unpatch_dict()
        return False

    def start(self):
        """Activate a patch, returning any created mock."""
        result = self.__enter__()
        _patch._active_patches.append(self)
        return result

    def stop(self):
        """Stop an active patch."""
        try:
            _patch._active_patches.remove(self)
        except ValueError:
            # If the patch hasn't been started this will fail
            return None

        return self.__exit__(None, None, None)


def _clear_dict(in_dict):
    try:
        in_dict.clear()
    except AttributeError:
        keys = list(in_dict)
        for key in keys:
            del in_dict[key]


def _patch_stopall():
    """Stop all active patches. LIFO to unroll nested patches."""
    for patch in reversed(_patch._active_patches):
        patch.stop()


patch.object = _patch_object
patch.dict = _patch_dict
patch.multiple = _patch_multiple
patch.stopall = _patch_stopall
patch.TEST_PREFIX = "test"
