"""Type lookup caches must preserve mutation, callbacks, and retirement."""


def warm(subject, name):
    result = None
    for _ in range(12):
        result = getattr(subject, name, None)
    return result


class Base:
    value = 'base-value'

    def method(self):
        return 'base-method'


class Child(Base):
    pass


class Peer(Base):
    def method(self):
        return 'peer-method'


def replacement(self):
    return 'replacement'


subject = Child()
selected = warm(subject, 'method')
print('initial', selected(), warm(subject, 'value'), warm(subject, 'added'))
Base.method = replacement
print('replace-base', selected(), warm(subject, 'method')())
del Base.value
print('delete-base', warm(subject, 'value'))
Base.added = 'new-value'
print('invalidate-miss', warm(subject, 'added'))
Child.method = Peer.method
print('replace-child', warm(subject, 'method')())
del Child.method
print('delete-child', warm(subject, 'method')())
subject.__class__ = Peer
print('class-change', warm(subject, 'method')(), warm(subject, 'added'))
subject.__class__ = Child
print('class-restore', warm(subject, 'method')())

retired = []


class Descriptor:
    def __get__(self, instance, owner):
        return 'descriptor'

    def __del__(self):
        retired.append('retired')


Base.expiring = Descriptor()
print('descriptor', warm(subject, 'expiring'))
del Base.expiring
print('retirement-before-lookup', len(retired))
print('retired-attribute', warm(subject, 'expiring'))

callbacks = []


class Key:
    def __hash__(self):
        return hash('missing')

    def __eq__(self, other):
        callbacks.append('comparison')
        raise RuntimeError('dictionary probe')


Watched = type('Watched', (), {Key(): None})
watched = Watched()
for _ in range(3):
    callbacks.clear()
    print('callback-error-miss', getattr(watched, 'missing', None), bool(callbacks))

# A dynamically constructed spelling still selects a known native method.
dynamic_builtin = ''.join(['up', 'per'])
print('dynamic-builtin', getattr('lower', dynamic_builtin)())

# Decoding a name for native dispatch must not replace the object passed to a
# Python attribute hook.
dynamic_name = ''.join(['obser', 'ved'])


class NameObserver:
    def __getattribute__(self, name):
        return name is dynamic_name


print('name-identity', getattr(NameObserver(), dynamic_name))

# Dictionary equality observes the actual query object. Derive its expected
# result from that identity, without assuming a particular interning policy.
# The space also exercises names outside the runtime's closed vocabulary.
first = ''.join(['query', ' name'])
second = ''.join(['query', ' name'])


class IdentityKey:
    def __hash__(self):
        return hash('query name')

    def __eq__(self, other):
        return other is first


IdentityWatched = type('IdentityWatched', (), {IdentityKey(): 17})
identity_watched = IdentityWatched()
print('equal-query-identity', warm(identity_watched, first) == 17,
      warm(identity_watched, second) == (17 if second is first else None),
      warm(identity_watched, first) == 17)

class ChangingKey:
    def __init__(self):
        self.enabled = False

    def __hash__(self):
        return hash('changing')

    def __eq__(self, other):
        return self.enabled


changing_key = ChangingKey()
Changing = type('Changing', (), {changing_key: 'selected'})
changing_subject = Changing()
print('changing-equality-miss', warm(changing_subject, 'changing'))
changing_key.enabled = True
print('changing-equality-hit', warm(changing_subject, 'changing'))
changing_key.enabled = False
print('changing-equality-miss-again', warm(changing_subject, 'changing'))

# The comparison key is not a type mutation. Publishing a class attribute is:
# it must invalidate both cached absence and cached presence from that key.
changing_key.enabled = True
Changing.refresh = 1
print('changing-after-class-set', warm(changing_subject, 'changing'))
changing_key.enabled = False
print('changing-cached-presence', warm(changing_subject, 'changing'))
del Changing.refresh
print('changing-after-class-delete', warm(changing_subject, 'changing'))
