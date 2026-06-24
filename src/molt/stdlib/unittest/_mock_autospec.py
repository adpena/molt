"""Autospec support for :mod:`unittest.mock`.

This module is loaded by ``mock.py`` with an explicit context module so the
public file can still be executed from an arbitrary file-location module name
by Molt's private-surface tests.
"""

import sys


_context = sys.modules[globals()["_MOLT_CONTEXT_MODULE"]]

inspect = _context.inspect
iscoroutinefunction = _context.iscoroutinefunction
InvalidSpecError = _context.InvalidSpecError
DEFAULT = _context.DEFAULT
ANY = _context.ANY
MagicMock = _context.MagicMock
AsyncMock = _context.AsyncMock
NonCallableMagicMock = _context.NonCallableMagicMock
_callable = _context._callable
_check_signature = _context._check_signature
_check_spec_arg_typos = _context._check_spec_arg_typos
_instance_callable = _context._instance_callable
_is_async_func = _context._is_async_func
_is_instance_mock = _context._is_instance_mock
_is_list = _context._is_list
_is_magic = _context._is_magic
_set_signature = _context._set_signature
_setup_async_mock = _context._setup_async_mock


def create_autospec(
    spec,
    spec_set=False,
    instance=False,
    _parent=None,
    _name=None,
    *,
    unsafe=False,
    **kwargs,
):
    """Create a mock object using another object as a spec. Attributes on the
    mock will use the corresponding attribute on the `spec` object as their
    spec.

    Functions or methods being mocked will have their arguments checked
    to check that they are called with the correct signature.

    If `spec_set` is True then attempting to set attributes that don't exist
    on the spec object will raise an `AttributeError`.

    If a class is used as a spec then the return value of the mock (the
    instance of the class) will have the same spec. You can use a class as the
    spec for an instance object by passing `instance=True`. The returned mock
    will only be callable if instances of the mock are callable.

    `create_autospec` will raise a `RuntimeError` if passed some common
    misspellings of the arguments autospec and spec_set. Pass the argument
    `unsafe` with the value True to disable that check.

    `create_autospec` also takes arbitrary keyword arguments that are passed to
    the constructor of the created mock."""
    if _is_list(spec):
        # can't pass a list instance to the mock constructor as it will be
        # interpreted as a list of strings
        spec = type(spec)

    is_type = isinstance(spec, type)
    if _is_instance_mock(spec):
        raise InvalidSpecError(f"Cannot autospec a Mock object. [object={spec!r}]")
    is_async_func = _is_async_func(spec)
    _kwargs = {"spec": spec}
    if spec_set:
        _kwargs = {"spec_set": spec}
    elif spec is None:
        # None we mock with a normal mock without a spec
        _kwargs = {}
    if _kwargs and instance:
        _kwargs["_spec_as_instance"] = True
    if not unsafe:
        _check_spec_arg_typos(kwargs)

    _name = kwargs.pop("name", _name)
    _new_name = _name
    if _parent is None:
        # for a top level object no _new_name should be set
        _new_name = ""

    _kwargs.update(kwargs)

    Klass = MagicMock
    if inspect.isdatadescriptor(spec):
        # descriptors don't have a spec
        # because we don't know what type they return
        _kwargs = {}
    elif is_async_func:
        if instance:
            raise RuntimeError(
                "Instance can not be True when create_autospec "
                "is mocking an async function"
            )
        Klass = AsyncMock
    elif not _callable(spec):
        Klass = NonCallableMagicMock
    elif is_type and instance and not _instance_callable(spec):
        Klass = NonCallableMagicMock

    mock = Klass(
        parent=_parent, _new_parent=_parent, _new_name=_new_name, name=_name, **_kwargs
    )

    if isinstance(spec, FunctionTypes):
        # should only happen at the top level because we don't
        # recurse for functions
        mock = _set_signature(mock, spec)
        if is_async_func:
            _setup_async_mock(mock)
    else:
        _check_signature(spec, mock, is_type, instance)

    if _parent is not None and not instance:
        _parent._mock_children[_name] = mock

    # Pop wraps from kwargs because it must not be passed to configure_mock.
    wrapped = kwargs.pop("wraps", None)
    if is_type and not instance and "return_value" not in kwargs:
        mock.return_value = create_autospec(
            spec, spec_set, instance=True, _name="()", _parent=mock, wraps=wrapped
        )

    for entry in dir(spec):
        if _is_magic(entry):
            # MagicMock already does the useful magic methods for us
            continue

        # XXXX do we need a better way of getting attributes without
        # triggering code execution (?) Probably not - we need the actual
        # object to mock it so we would rather trigger a property than mock
        # the property descriptor. Likewise we want to mock out dynamically
        # provided attributes.
        # XXXX what about attributes that raise exceptions other than
        # AttributeError on being fetched?
        # we could be resilient against it, or catch and propagate the
        # exception when the attribute is fetched from the mock
        try:
            original = getattr(spec, entry)
        except AttributeError:
            continue

        child_kwargs = {"spec": original}
        # Wrap child attributes also.
        if wrapped and hasattr(wrapped, entry):
            child_kwargs.update(wraps=original)
        if spec_set:
            child_kwargs = {"spec_set": original}

        if not isinstance(original, FunctionTypes):
            new = _SpecState(original, spec_set, mock, entry, instance)
            mock._mock_children[entry] = new
        else:
            parent = mock
            if isinstance(spec, FunctionTypes):
                parent = mock.mock

            skipfirst = _must_skip(spec, entry, is_type)
            child_kwargs["_eat_self"] = skipfirst
            if iscoroutinefunction(original):
                child_klass = AsyncMock
            else:
                child_klass = MagicMock
            new = child_klass(
                parent=parent,
                name=entry,
                _new_name=entry,
                _new_parent=parent,
                **child_kwargs,
            )
            mock._mock_children[entry] = new
            new.return_value = child_klass()
            _check_signature(original, new, skipfirst=skipfirst)

        # so functions created with _set_signature become instance attributes,
        # *plus* their underlying mock exists in _mock_children of the parent
        # mock. Adding to _mock_children may be unnecessary where we are also
        # setting as an instance attribute?
        if isinstance(new, FunctionTypes):
            setattr(mock, entry, new)
    # kwargs are passed with respect to the parent mock so, they are not used
    # for creating return_value of the parent mock. So, this condition
    # should be true only for the parent mock if kwargs are given.
    if _is_instance_mock(mock) and kwargs:
        mock.configure_mock(**kwargs)

    return mock


def _must_skip(spec, entry, is_type):
    """
    Return whether we should skip the first argument on spec's `entry`
    attribute.
    """
    if not isinstance(spec, type):
        if entry in getattr(spec, "__dict__", {}):
            # instance attribute - shouldn't skip
            return False
        spec = spec.__class__

    for klass in spec.__mro__:
        result = klass.__dict__.get(entry, DEFAULT)
        if result is DEFAULT:
            continue
        if isinstance(result, (staticmethod, classmethod)):
            return False
        elif isinstance(result, FunctionTypes):
            # Normal method => skip if looked up on type
            # (if looked up on instance, self is already skipped)
            return is_type
        else:
            return False

    # function is a dynamically provided attribute
    return is_type


class _SpecState(object):
    def __init__(
        self, spec, spec_set=False, parent=None, name=None, ids=None, instance=False
    ):
        self.spec = spec
        self.ids = ids
        self.spec_set = spec_set
        self.parent = parent
        self.instance = instance
        self.name = name


FunctionTypes = (
    # python function
    type(create_autospec),
    # instance method
    type(ANY.__eq__),
)
