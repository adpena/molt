from functools import wraps

def decorator(func):
    @wraps(func)
    def wrapper(*args, **kwargs):
        print(f"Calling {func.__name__}")
        return func(*args, **kwargs)
    return wrapper

@decorator
def greet(name):
    '''Greet someone by name.'''
    print(f"Hello, {name}!")

greet("World")
print(greet.__name__)
print(greet.__doc__)
