class User:
    id: int

u = User()
u.id = 123
# We'll simulate 'uncertain' type by just using the object
val = u.id 
print(val)