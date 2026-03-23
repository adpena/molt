import flask

print("flask", flask.__version__)
print("Flask exists:", hasattr(flask, "Flask"))
print("Blueprint exists:", hasattr(flask, "Blueprint"))
print("jsonify exists:", hasattr(flask, "jsonify"))
