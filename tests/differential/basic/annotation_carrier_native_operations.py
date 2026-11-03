class Owner:
    def __add__(self, other): return self
    def __sub__(self, other): return self
    def __mul__(self, other): return self
    def __truediv__(self, other): return self
    def __floordiv__(self, other): return self
    def __mod__(self, other): return self
    def __pow__(self, other): return self
    def __neg__(self): return self
    def __pos__(self): return self
    def __lt__(self, other): return self
    def __eq__(self, other): return self

def add(x: float, y: float): return x + y
def sub(x: float, y: float): return x - y
def mul(x: float, y: float): return x * y
def div(x: float, y: float): return x / y
def floordiv(x: float, y: float): return x // y
def mod(x: float, y: float): return x % y
def power(x: float, y: float): return x ** y
def negative(x: float, y: float): return -x
def positive(x: float, y: float): return +x
def compare(x: float, y: float): return x < y
def equal(x: float, y: float): return x == y

value = add(2, 3)
print('integer-add', type(value).__name__, value)
for function in (add, sub, mul, div, floordiv, mod, power, negative, positive, compare, equal):
    owner = Owner()
    try:
        result = function(owner, owner)
        print(function.__name__, result is owner, type(result).__name__)
    except Exception as error:
        print(function.__name__, type(error).__name__)

# In-place selectors and truth branches share the annotation boundary.
def inplace_add(x: float, y: float):
    x += y
    return x

def inplace_sub(x: float, y: float):
    x -= y
    return x

def inplace_mul(x: float, y: float):
    x *= y
    return x

def inplace_div(x: float, y: float):
    x /= y
    return x

def inplace_floor(x: float, y: float):
    x //= y
    return x

def inplace_mod(x: float, y: float):
    x %= y
    return x

def inplace_power(x: float, y: float):
    x **= y
    return x

def power_unknown_base(x, y: float):
    return x ** y

for function in (inplace_add, inplace_sub, inplace_mul, inplace_div,
                 inplace_floor, inplace_mod, inplace_power, power_unknown_base):
    owner = Owner()
    result = function(owner, 2.0)
    print(function.__name__, result is owner)

def truth_branch(x: bool):
    if x:
        return 'yes'
    return 'no'

def value_and(x: bool, y: bool):
    return x and y

def value_or(x: bool, y: bool):
    return x or y

for value in (0, 2, 4, -2, False, True):
    print('branch', value, truth_branch(value), value_and(value, 9), value_or(value, 9))

class InPlaceOwner:
    def __iadd__(self, other): print('iadd'); return self
    def __isub__(self, other): print('isub'); return self
    def __imul__(self, other): print('imul'); return self
    def __itruediv__(self, other): print('idiv'); return self
    def __ifloordiv__(self, other): print('ifloor'); return self
    def __imod__(self, other): print('imod'); return self
    def __ipow__(self, other): print('ipow'); return self

for function in (inplace_add, inplace_sub, inplace_mul, inplace_div,
                 inplace_floor, inplace_mod, inplace_power):
    owner = InPlaceOwner()
    result = function(owner, 2.0)
    print('in-place-identity', result is owner)
