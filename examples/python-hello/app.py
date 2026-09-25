"""Minimal Python web service for Ferry (standard library only)."""
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"ok" if self.path == "/healthz" else (
            f"Hello from Ferry (python)! service={os.environ.get('FERRY_SERVICE_NAME', '?')}\n".encode()
        )
        self.send_response(200)
        self.send_header("content-type", "text/plain")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "8000"))
    print(f"python-hello listening on :{port}", flush=True)
    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()
