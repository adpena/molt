"""Float quotient/remainder invariants and version-specific complex mixed modes."""
values = (0.0, -0.0, 1.0, -1.0, 7.0, -7.0, 0.1, -0.1, 1e-300, -1e-300, 1e300, -1e300, float("inf"), float("-inf"), float("nan"))
for a in values:
    for b in values:
        for operation in ("floor", "mod", "divmod"):
            try:
                result = a//b if operation == "floor" else a%b if operation == "mod" else divmod(a,b)
                print(operation, repr(a), repr(b), repr(result))
            except Exception as error:
                print(operation, repr(a), repr(b), type(error).__name__, str(error))
complexes = (0j, complex(-0.0,0.0), complex(0.0,-0.0), 1+2j, -1+2j, complex(1e-300,0.0), complex(0.0,1e-300), complex(1e300,1e300), complex(float("inf"),0.0), complex(0.0,float("inf")), complex(float("nan"),1.0), complex(1.0,float("nan")))
for a in values + complexes:
    for b in values + complexes:
        if not isinstance(a,complex) and not isinstance(b,complex):
            continue
        try:
            print("complexdiv", repr(a), repr(b), repr(a/b))
        except Exception as error:
            print("complexdiv", repr(a), repr(b), type(error).__name__, str(error))

# Huge integers must overflow at real/complex coercion before arithmetic zero
# checks; exact integer ratios and ordering deliberately avoid that coercion.
huge = 10**400
for operation in ("add", "sub", "mul", "div", "floor", "mod", "divmod", "pow"):
    for label, a, b in (("big-real", huge, 1.0), ("real-big", 1.0, huge),
                        ("big-zero", huge, 0.0), ("big-inf", huge, float("inf")),
                        ("big-nan", huge, float("nan"))):
        try:
            if operation == "add": result = a+b
            elif operation == "sub": result = a-b
            elif operation == "mul": result = a*b
            elif operation == "div": result = a/b
            elif operation == "floor": result = a//b
            elif operation == "mod": result = a%b
            elif operation == "divmod": result = divmod(a,b)
            else: result = a**b
            print("coercion", operation, label, repr(result))
        except Exception as error:
            print("coercion", operation, label, type(error).__name__, str(error))
for label, a, b in (("big-negative", huge, -1), ("zero-huge-negative", 0, -huge),
                    ("big-complex", huge, 1+0j), ("complex-big", 1+0j, huge)):
    try:
        print("power-coercion", label, repr(a**b))
    except Exception as error:
        print("power-coercion", label, type(error).__name__, str(error))
print("exact-ratio", huge/huge)
print("ordering", huge<float("inf"), huge==float("nan"))

for label, integer in (("zero", 0), ("negative", -1), ("boundary", 2**53+1), ("huge", huge)):
    for value in (0.0, -0.0, 0.5, -0.5, -1.0, float(2**53), float("inf"), float("-inf"), float("nan")):
        real_complex = complex(value, -0.0)
        print("exact-compare", label, repr(value), integer==value, value==integer,
              integer<value, integer>value, integer==real_complex, real_complex==integer)
print("real-complex-equality", 7.0==7+0j, 7+0j==7.0, 0==1j, 1j==0)

class IntegerChild(int):
    pass
class FloatChild(float):
    pass
class ReflectedInteger(int):
    def __eq__(self, other):
        return "reflected-equality"
for integer in (IntegerChild(2**53+1), IntegerChild(huge)):
    for real in (FloatChild(float(2**53)), FloatChild(float("inf")), FloatChild(float("nan"))):
        print("subclass-compare", integer==real, real==integer, integer<real, integer>real)
print("reflected-subclass", 1.0==ReflectedInteger(1), ReflectedInteger(1)==1.0)

for exponent_label, exponent in (("zero", 0), ("one", 1), ("machine-wide", 2**63),
                                 ("huge-even", huge), ("huge-odd", huge+1)):
    for base in (0, 1, -1, False, True):
        result = base**exponent
        inplace = base
        inplace **= exponent
        print("integer-power-identity", exponent_label, repr(base), repr(result), repr(inplace))
print("huge-base-zero-power", huge**0)

for base in (complex(1e-300,0.0), complex(-1e-300,0.0), complex(1e300,0.0),
             complex(-1e300,0.0), -1e-300, -1e300):
    for exponent in (0.5,-0.5,0j,complex(0.5,0.0)):
        try:
            print("complex-power-magnitude", repr(base), repr(exponent), repr(base**exponent))
        except Exception as error:
            print("complex-power-magnitude", repr(base), repr(exponent), type(error).__name__, str(error))
for base in (complex(float("inf"),0.0), complex(float("nan"),1.0), complex(0.0,-0.0)):
    print("complex-zero-power", repr(base), repr(base**complex(-0.0,0.0)))
