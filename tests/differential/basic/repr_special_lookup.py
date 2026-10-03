"""Implicit repr/str bind on the type, bypassing instance attribute hooks."""

import types


def rendered_repr(self):
    return 'class-repr'


def rendered_str(self):
    return 'class-str'


def intercept(self, name):
    if name in ('__repr__', '__str__'):
        raise AssertionError('implicit special method used __getattribute__')
    return object.__getattribute__(self, name)


def invalid_result(self):
    return 7


def raise_result(self):
    raise ValueError('format-failure')


def observe_error(label, operation):
    try:
        operation()
    except Exception as error:
        print(label, type(error).__name__, str(error))
    else:
        print(label, 'no error')


for base, args in (
    (object, ()),
    (list, ([1],)),
    (tuple, ((1,),)),
    (dict, ()),
    (set, ()),
    (frozenset, ()),
    (types.ModuleType, ('example',)),
    (Exception, ('example',)),
):
    cls = type('Rendered', (base,), {
        '__repr__': rendered_repr,
        '__str__': rendered_str,
        '__getattribute__': intercept,
    })
    instance = cls(*args)
    instance.__repr__ = lambda: 'instance-repr'
    instance.__str__ = lambda: 'instance-str'
    print(base.__name__, repr(instance), str(instance))
    for name, operation in (('__repr__', repr), ('__str__', str)):
        setattr(cls, name, invalid_result)
        observe_error(base.__name__ + name + '-invalid', lambda: operation(instance))
        setattr(cls, name, raise_result)
        observe_error(base.__name__ + name + '-raise', lambda: operation(instance))


class ReprOnly(types.ModuleType):
    __repr__ = rendered_repr
    __getattribute__ = intercept


module = ReprOnly('repr-only')
module.__repr__ = lambda: 'instance-repr'
module.__str__ = lambda: 'instance-str'
print('default-str', repr(module), str(module))


class RenderDescriptor:
    def __get__(self, instance, owner):
        print('bind', owner.__name__, instance is not None)
        return lambda: 'descriptor-result'


class DescriptorModule(types.ModuleType):
    __repr__ = RenderDescriptor()
    __str__ = RenderDescriptor()
    __getattribute__ = intercept


module = DescriptorModule('descriptor')
print('descriptor-repr', repr(module))
print('descriptor-str', str(module))


class RaisingDescriptor:
    def __get__(self, instance, owner):
        raise ValueError('binding-failure')


DescriptorModule.__repr__ = RaisingDescriptor()
DescriptorModule.__str__ = RaisingDescriptor()
observe_error('repr-bind', lambda: repr(module))
observe_error('str-bind', lambda: str(module))
