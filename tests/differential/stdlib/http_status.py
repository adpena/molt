from http import HTTPStatus

print(HTTPStatus.OK)
print(HTTPStatus.OK.value)
print(HTTPStatus.OK.phrase)
print(HTTPStatus.NOT_FOUND)
print(HTTPStatus.NOT_FOUND.value)
print(HTTPStatus.NOT_FOUND.phrase)
print(HTTPStatus(200) == HTTPStatus.OK)
print(HTTPStatus(404).name)

# List some common ones
for code in [200, 201, 301, 400, 403, 404, 500]:
    s = HTTPStatus(code)
    print(f"{s.value} {s.phrase}")
