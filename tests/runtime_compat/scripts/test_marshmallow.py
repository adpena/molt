import marshmallow
from marshmallow import Schema, fields


class UserSchema(Schema):
    name = fields.Str(required=True)
    age = fields.Int()


schema = UserSchema()
result = schema.load({"name": "Alice", "age": 30})
print("marshmallow", marshmallow.__version__)
print("loaded:", result)
