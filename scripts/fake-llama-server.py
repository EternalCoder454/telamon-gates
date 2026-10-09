#!/usr/bin/env python3
"""A stand-in for llama-server, for scripts/watchdog.sh and scripts/screens.sh:
answers /health and /v1/models, and streams a long reply, one word at a time,
so a reply is under way while the script changes the graphics memory. Takes
llama-server's --port; ignores the rest. Quits on SIGTERM, as llama-server
does."""
import json
import signal
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

WORDS = "Lifetimes name how long a reference stays valid. ".split()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/health":
            self.reply({"status": "ok"})
        elif self.path == "/v1/models":
            self.reply({"data": [{"id": "test", "meta": {"n_ctx": 8192}}]})
        else:
            self.send_error(404)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            for i in range(600):
                chunk = {"choices": [{"delta": {"content": WORDS[i % len(WORDS)] + " "}}]}
                self.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n")
                self.wfile.flush()
                time.sleep(0.1)
            self.wfile.write(b"data: [DONE]\n\n")
        except (BrokenPipeError, ConnectionResetError):
            pass


def main():
    port = int(sys.argv[sys.argv.index("--port") + 1])
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.daemon_threads = True
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    server.serve_forever()


main()
