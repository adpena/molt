"""Observe the ModuleType representation and protocol boundary."""

import gc
import types
import weakref


def snapshot(label, module):
    print(label, type(module).__name__, isinstance(module, types.ModuleType))
    print([(key, module.__dict__.get(key, '<missing>')) for key in
           ('__name__', '__doc__', '__package__', '__loader__', '__spec__')])


class Inherited(types.ModuleType):
    pass


class Lazy(types.ModuleType):
    def __init__(self, name, doc=None):
        super().__init__(name, doc)
        self.loaded = False

    def __getattr__(self, name):
        if name == 'answer':
            self.loaded = True
            return 42
        raise AttributeError(name)

    def __dir__(self):
        return ['answer', '__name__']

    def __repr__(self):
        return 'lazy-module:' + self.__name__


snapshot('exact', types.ModuleType('exact'))
snapshot('inherited', Inherited('inherited', 'documentation'))
lazy = Lazy('lazy', 'doc')
snapshot('lazy', lazy)
print(lazy.answer, lazy.loaded, dir(lazy), repr(lazy))
lazy.__dict__['__getattr__'] = lambda name: 'namespace:' + name
print(lazy.dynamic)
lazy.preserved = 9
types.ModuleType.__init__(lazy, 'reinitialized')
snapshot('reinitialized', lazy)
print(lazy.preserved, lazy.loaded)

uninitialized = types.ModuleType.__new__(Inherited)
print('uninitialized', type(uninitialized).__name__, sorted(uninitialized.__dict__))
types.ModuleType.__init__(uninitialized, name='named', doc='keyword-doc')
snapshot('initialized', uninitialized)

events = []


class Descriptors(types.ModuleType):
    @property
    def value(self):
        return self.__dict__.get('_value', 10)

    @value.setter
    def value(self, value):
        events.append(('set-property', value))
        self.__dict__['_value'] = value

    @value.deleter
    def value(self):
        events.append(('delete-property',))
        del self.__dict__['_value']

    def method(self):
        return self.__name__

    def __setattr__(self, name, value):
        events.append(('setattr', name))
        super().__setattr__(name, value)

    def __delattr__(self, name):
        events.append(('delattr', name))
        super().__delattr__(name)


descriptor = Descriptors('descriptors')
descriptor.__dict__['value'] = 99
print('descriptor', descriptor.value, descriptor.method())
descriptor.value = 12
print('assigned', descriptor.value, descriptor.__dict__['value'])
del descriptor.value
print('deleted', descriptor.value, events)
plain = types.ModuleType('reclassed')
plain.__class__ = Descriptors
print('reclassed', type(plain).__name__, plain.value, plain.method())


class Slotted(types.ModuleType):
    __slots__ = ('slot',)


slotted = Slotted('slotted')
slotted.slot = 17
slotted.extra = 23
print('slots', slotted.slot, slotted.extra, 'slot' in slotted.__dict__)
reference = weakref.ref(slotted)
slotted.cycle = slotted
del slotted
gc.collect()
print('collected', reference() is None)


class Different(types.ModuleType):
    def __new__(cls, *args, **kwargs):
        return 27

    def __init__(self, *args, **kwargs):
        raise AssertionError('initializer must be skipped')


print('different', Different('unused'))
for arguments in ((), (1,), ('a', None, 'extra')):
    try:
        types.ModuleType(*arguments)
    except Exception as error:
        print('invalid', type(error).__name__, str(error))
