"""Echo service for tests: GET / → hello, /healthz → ok, /env?key=NAME → value of NAME."""
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/healthz":
            body = b"ok"
        elif url.path == "/env":
            key = parse_qs(url.query).get("key", [""])[0]
            body = json.dumps({"key": key, "value": os.environ.get(key)}).encode()
        elif url.path == "/headers":
            body = json.dumps(dict(self.headers)).encode()
        else:
            body = f"echo from {os.environ.get('FERRY_SERVICE_NAME', '?')} ({os.environ.get('HOSTNAME', '?')})\n".encode()
        self.send_response(200)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    print(f"docker-echo listening on :{port}", flush=True)
    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()
