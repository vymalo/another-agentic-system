"""A stub upstream for tests/files-host-ci.sh: it stands where RustFS stands, answers every request with 200, and logs what REACHED it.

    python3 files-host-stub.py <port> <log file>

A request the proxy refuses never gets here, so the log is the evidence of what the proxy let through. Each request that does is two JSON lines:
"start" the moment its method, target and headers have arrived (before any byte of the body is read), and "end" once the body has been read, or
has been cut off by the proxy, with the number of body bytes received and whether the body ended as declared. The first is what says "reached"
(it is written long before the proxy can answer the client); the second is what says how much.

It answers with headers a real upstream could send and the proxy must not pass on: a Server, an Access-Control-Allow-Origin for another origin.
Standard library only; listens on all interfaces (the proxy runs in a container and reaches it through the host gateway).
"""
import http.server
import json
import sys

PORT = int(sys.argv[1])
LOG = open(sys.argv[2], "a", buffering=1, encoding="utf-8")


def read_body(rfile, headers):
    """Returns (bytes received, whether the body ended as declared)."""
    received = 0
    try:
        if "chunked" in headers.get("Transfer-Encoding", "").lower():
            while True:
                size_line = rfile.readline().strip()
                if not size_line:
                    return received, False
                size = int(size_line.split(b";")[0], 16)
                if size == 0:
                    rfile.readline()
                    return received, True
                chunk = rfile.read(size)
                received += len(chunk)
                rfile.readline()
                if len(chunk) < size:
                    return received, False
        left = int(headers.get("Content-Length") or 0)
        while left > 0:
            data = rfile.read(min(65536, left))
            if not data:
                return received, False
            received += len(data)
            left -= len(data)
        return received, True
    except (OSError, ValueError):
        return received, False


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "stub-upstream"
    sys_version = ""
    timeout = 10

    def record(self, phase, received, complete):
        line = {
            "phase": phase,
            "method": self.command,
            "target": self.path,
            "body_bytes": received,
            "complete": complete,
            "headers": {k.lower(): v for k, v in self.headers.items()},
        }
        LOG.write(json.dumps(line, sort_keys=True, separators=(",", ":")) + "\n")

    def handle_any(self):
        self.record("start", 0, False)
        received, complete = read_body(self.rfile, self.headers)
        self.record("end", received, complete)
        body = b"stub-upstream-reached\n"
        try:
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Type", "text/plain")
            self.send_header("Access-Control-Allow-Origin", "https://evil.example")
            self.send_header("Access-Control-Allow-Credentials", "true")
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(body)
        except OSError:
            pass

    do_GET = do_PUT = do_POST = do_DELETE = do_PATCH = do_HEAD = do_OPTIONS = do_TRACE = do_PROPFIND = handle_any

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
