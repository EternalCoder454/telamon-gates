"""What the eval scripts share: starting llama-server with the flags given on
the command line, waiting until the model is loaded, and asking it for a
reply. Standard library only."""
import contextlib
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.request

# telamon-llama's server, else one on $PATH; LLAMA_SERVER overrides both.
PACKAGED = "/usr/libexec/telamon-llama/llama-server"
# Where results go (failed answers, saved replies, server logs).
OUT = os.environ.get("EVAL_OUT", "out/eval")
# Context for every run, in tokens (-c); a flag in the arguments wins.
CONTEXT = os.environ.get("EVAL_CTX", "16384")
# A cap on every reply's tokens (0 for none), to try the scripts on a tiny
# model on the processor; the scores then mean nothing.
CAP = int(os.environ.get("EVAL_MAX_TOKENS", "0"))


def max_tokens(n):
    return min(n, CAP) if CAP else n


def find_server():
    path = os.environ.get("LLAMA_SERVER") or (PACKAGED if os.path.exists(PACKAGED) else shutil.which("llama-server"))
    if not path:
        sys.exit("eval: no llama-server: install the telamon-llama RPM, or set LLAMA_SERVER=<path>")
    return path


def need_model(args):
    """The model comes from the caller's arguments (-m/--model, or -hf), never
    from this repository."""
    if not any(a in ("-m", "--model", "-hf", "--hf-repo") or a.startswith(("--model=", "--hf-repo=")) for a in args):
        sys.exit("eval: the llama-server arguments must name a model: -m <file.gguf>")


def out_path(*parts):
    path = os.path.join(OUT, *parts)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    return path


@contextlib.contextmanager
def server(name, args, port, wait=300):
    """llama-server on 127.0.0.1:`port` with `args`, once /health says ok;
    stopped on exit. Returns the seconds the model took to load."""
    need_model(args)
    log = open(out_path("logs", f"{name}.log"), "w")
    proc = subprocess.Popen(
        [find_server(), "--port", str(port), "--host", "127.0.0.1", "-c", CONTEXT, "--jinja", *args],
        stdout=log, stderr=subprocess.STDOUT)
    started = time.time()
    try:
        while True:
            if proc.poll() is not None:
                sys.exit(f"eval: llama-server stopped ({proc.returncode}); see {log.name}")
            try:
                if b"ok" in urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2).read():
                    break
            except Exception:
                pass
            if time.time() - started > wait:
                sys.exit(f"eval: the model did not load in {wait} s; see {log.name}")
            time.sleep(0.5)
        yield time.time() - started
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        log.close()


def chat(port, body, timeout=900):
    """POST /v1/chat/completions; returns the decoded answer and the wall time."""
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    started = time.time()
    answer = json.load(urllib.request.urlopen(req, timeout=timeout))
    return answer, time.time() - started


def reasoning(think):
    """The request fields for Qwen3's thinking switch and gpt-oss's effort.
    With think off they are the two Gates sends for a brief reply
    (Request::brief); other chat templates ignore both."""
    return {"chat_template_kwargs": {"enable_thinking": think},
            "reasoning_effort": "medium" if think else "low"}
