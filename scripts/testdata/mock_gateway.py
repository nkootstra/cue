#!/usr/bin/env python3
"""Mock gateway for cue's end-to-end verification.

Serves both surfaces cue talks to:
- Ollama native API: GET /api/tags, POST /api/chat, POST /api/pull,
  POST /api/create
- OpenAI-compatible: POST /v1/chat/completions

Normalization prompts (containing the S1 control line) get cleaned text;
anything else is treated as an analysis request and answered with a fixed
analysis JSON.
"""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

ANALYSIS = {
    "language": "en",
    "title": "Cue Pipeline Test",
    "summary": "A spoken sentence describing the cue pipeline.",
    "topics": [
        {"start_ms": 0, "end_ms": 3000, "title": "Intro",
         "summary": "The greeting.", "key_points": ["says hello"]}
    ],
    "key_points": ["one point", "two points"],
    "keywords": ["test", "cue"],
}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _json(self, payload):
        body = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/api/tags":
            self._json({"models": [{"name": "cue-s1-mini"}]})
        else:
            self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length).decode()
        if "/api/pull" in self.path or "/api/create" in self.path:
            self._json({"status": "success"})
            return
        if self.path == "/api/chat":
            request = json.loads(raw)
            messages = request.get("messages", [])
            valid_prompt = (
                len(messages) == 1
                and messages[0].get("role") == "user"
                and messages[0].get("content", "").startswith("[Styling:")
            )
            if (
                request.get("model") != "cue-s1-mini"
                or request.get("stream") is not False
                or request.get("think") is not False
                or not valid_prompt
            ):
                self.send_error(400, "invalid S1 normalization request")
                return
            self._json({
                "message": {
                    "role": "assistant",
                    "content": "Hello, this is a test of the Q transcription pipeline.",
                },
                "done": True,
            })
            return
        if "/v1/chat/completions" not in self.path:
            self.send_error(404)
            return
        content = json.dumps(ANALYSIS)
        self._json({
            "choices": [{"message": {"role": "assistant", "content": content}}]
        })


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", 8765), Handler).serve_forever()
