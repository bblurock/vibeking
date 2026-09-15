#!/usr/bin/env python3
"""Stateful streaming Gemma sidecar — HTTP wrapper around GemmaSession.

Replaces the stateless `mlx_vlm.server` for vibeking's Gemma live-preview path.
One model is loaded at startup; each recording opens a session that holds a
persistent KV cache and prefills only NEW audio per chunk (see gemma_session.py).

Single-threaded on purpose: MLX generation is serialized, and vibeking drives one
recording at a time, sending chunks sequentially. Loopback only.

Endpoints
  GET  /health                      -> {"ok": true, "model": "..."}
  POST /session/start               -> {"session_id": "..."}
       body (optional JSON): {"instruction": "...", "keepback": 2}
  POST /session/<id>/feed           body: raw f32le PCM @16k (the NEW chunk)
                                    -> {"text": "<partial transcript>"}
  POST /session/<id>/finish         -> {"text": "<authoritative transcript>"}
       (also drops the session)
  POST /session/<id>/cancel         -> {"ok": true}   (drop without finishing)

Audio wire format: little-endian float32 mono @ 16 kHz, raw bytes in the request
body. (vibeking already captures f32 PCM, so no transcoding.)
"""
import argparse
import json
import sys
import numpy as np
import mlx.core as mx
from http.server import BaseHTTPRequestHandler, HTTPServer

from mlx_vlm import load
from mlx_vlm.utils import load_config
from gemma_session import GemmaSession

# MLX pools freed Metal buffers and never returns them to the OS; the pool is
# unbounded by default. Because each feed re-encodes a GROWING audio buffer,
# allocation shapes rarely repeat, so the pool accumulates instead of reusing
# (observed at 36 GB of IOAccelerator memory in the wild). Bound the pool, and
# drop it entirely between recordings — the next session re-warms in one tick.
CACHE_LIMIT_BYTES = 2 * 1024**3

# Single-threaded on purpose: MLX is thread-affine (inference on a request
# thread fails with "no Stream(gpu) in current thread"), so requests serialize.
# The app must therefore NOT treat a briefly-busy server as dead — it reuses
# based on our tracked child PID being alive, not a /health ping that can queue
# behind an in-flight decode (see gemma_server.rs reuse logic).

# Populated in main().
MODEL = None
PROCESSOR = None
CONFIG = None
MODEL_ID = None
SESSIONS = {}
_next_id = [0]


def _new_session(instruction=None, keepback=2):
    _next_id[0] += 1
    sid = f"s{_next_id[0]}"
    kw = {"keepback_tokens": keepback}
    if instruction:
        kw["instruction"] = instruction
    SESSIONS[sid] = GemmaSession(MODEL, PROCESSOR, CONFIG, **kw)
    return sid


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):  # quiet; vibeking captures stderr separately
        pass

    def _send(self, code, obj):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _body(self):
        n = int(self.headers.get("Content-Length", 0))
        return self.rfile.read(n) if n else b""

    def do_GET(self):
        if self.path == "/health":
            self._send(200, {"ok": True, "model": MODEL_ID})
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        parts = [p for p in self.path.split("/") if p]
        try:
            if parts == ["session", "start"]:
                raw = self._body()
                opts = json.loads(raw) if raw else {}
                sid = _new_session(opts.get("instruction"), int(opts.get("keepback", 2)))
                self._send(200, {"session_id": sid})
                return

            if len(parts) == 3 and parts[0] == "session":
                sid, action = parts[1], parts[2]
                sess = SESSIONS.get(sid)
                if sess is None:
                    self._send(404, {"error": f"unknown session {sid}"})
                    return

                if action == "feed":
                    pcm = np.frombuffer(self._body(), dtype="<f4")
                    text = sess.feed(pcm)
                    self._send(200, {"text": text})
                    return
                if action == "finish":
                    self._body()  # drain
                    text = sess.finish()
                    SESSIONS.pop(sid, None)
                    mx.clear_cache()  # release the recording's Metal buffer pool
                    self._send(200, {"text": text})
                    return
                if action == "cancel":
                    self._body()
                    SESSIONS.pop(sid, None)
                    mx.clear_cache()
                    self._send(200, {"ok": True})
                    return

            self._send(404, {"error": "not found"})
        except Exception as e:  # never let one bad request kill the server
            self._send(500, {"error": f"{type(e).__name__}: {e}"})


def main():
    global MODEL, PROCESSOR, CONFIG, MODEL_ID
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=51247)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--model", default="mlx-community/gemma-4-e4b-it-4bit")
    args = ap.parse_args()

    MODEL_ID = args.model
    mx.set_cache_limit(CACHE_LIMIT_BYTES)
    print(f"[gemma-stream] loading {MODEL_ID} ...", file=sys.stderr, flush=True)
    MODEL, PROCESSOR = load(MODEL_ID)
    CONFIG = load_config(MODEL_ID)
    print(f"[gemma-stream] ready on {args.host}:{args.port}", file=sys.stderr, flush=True)

    HTTPServer((args.host, args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
