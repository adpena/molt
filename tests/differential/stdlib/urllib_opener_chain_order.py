"""Purpose: differential coverage for urllib opener chain ordering."""

import urllib.request

log: list[str] = []


class FirstHandler(urllib.request.BaseHandler):
    handler_order = 100

    def http_open(self, req):
        log.append("first")
        return self.parent.open(req)


class SecondHandler(urllib.request.BaseHandler):
    handler_order = 200

    def http_open(self, req):
        log.append("second")
        return self.parent.open(req)


opener = urllib.request.OpenerDirector()
opener.add_handler(SecondHandler())
opener.add_handler(FirstHandler())

req = urllib.request.Request("data:,ok")
with opener.open(req) as resp:
    _ = resp.read()

print(log)
