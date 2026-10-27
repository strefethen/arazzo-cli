#!/usr/bin/env python3
"""Demo responder for examples/kafka-order-followup.arazzo.yaml.

Answers GET /orders/<id> on 127.0.0.1:18080 with {"orderId": <id>, "status":
"placed"} so the example's dependent HTTP step has something to call. Standard
library only; stop it with Ctrl-C.
"""
import json
import re
from http.server import BaseHTTPRequestHandler, HTTPServer

ORDER = re.compile(r"^/orders/(\d+)$")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802 — http.server naming
        match = ORDER.match(self.path)
        status = 200 if match else 404
        body = (
            {"orderId": int(match.group(1)), "status": "placed"}
            if match
            else {"error": "not found"}
        )
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt, *args):
        print(f"order-server: {self.command} {self.path} -> {args[1]}", flush=True)


if __name__ == "__main__":
    print("order-server: listening on http://127.0.0.1:18080", flush=True)
    HTTPServer(("127.0.0.1", 18080), Handler).serve_forever()
