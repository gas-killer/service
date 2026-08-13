#!/usr/bin/env python3
"""gk-shard-bridge: public ask -> sharded inference -> single on-chain settlement.

POST /bridge/ask {"model":"qwen"|"qwen35","prompt_ids":[...],"max_new":8}
  -> {"ask_id": "..."} immediately; work continues in a thread:
     1. POST the router's internal shard coordinator (/shard/infer) — the
        inference runs as hash-committed segments across the operator committee.
     2. When the answer lands, build fulfil(promptIds,maxNew,answerIds,root)
        calldata and submit ONE settlement round via the router's public
        /trigger. The operator gate re-verifies the segment commit chain before
        signing; the quorum's verifyAndUpdate lands the answer on Sepolia.
GET /bridge/ask/<id> -> status JSON (inferring -> settling -> done | error)
GET /bridge/healthz  -> ok
"""
import json
import os
import threading
import time
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SHARD = os.environ.get("ROUTER_SHARD", "http://gas-killer-router:8081")
API = os.environ.get("ROUTER_API", "http://gas-killer-router:8080")
# The operators simulate the settlement task against the qwen-sim-fork anvil,
# which lags the live chain until its periodic re-fork — a task pinned at
# live_head-3 can reference a block the fork doesn't have yet ("block don't
# exists" → the round dies). Pin at the FORK's head instead (gk-sim-proxy
# serves it); BLOCK_STALE_MEASURE=50000 keeps that fresh enough on-chain.
FORKHEAD = os.environ.get("FORKHEAD_URL", "http://gk-sim-proxy:8545/forkhead")
GK_KEY = os.environ["GK_KEY"]
RPC = os.environ.get("RPC_URL", "https://ethereum-sepolia-rpc.publicnode.com")
FROM_ADDR = os.environ.get("FROM_ADDR", "0x6636A1CCBdf54485067304C1a590DE016DeaD9F0")

SEL_FULFIL = "9c98c06e"  # fulfil(uint32[],uint256,uint32[],bytes32)
SEL_COUNT = "0xf4833e20"  # stateTransitionCount()

MODELS = {
    "qwen": {
        "consumer": "0xd3f7F985F14f1942Fb09e5735e5499FEFF56E80b",
        "req": {
            "consumer": "0xd3f7F985F14f1942Fb09e5735e5499FEFF56E80b",
            "seg_engine": "0x18C8b1677a731f7507ea51D99e23e513D9613Aa4",
            "weights_root": "0x0000000000000000000000000000000000000000",
            "manifest": "0x23216cb9ed9ef2b4bc20c84d27b68fa62ab194fc0845dfa707836f48ec4a7ae9",
            "packed_config": [
                "0x04000c001c100800800002518004000101000000000000000000000000000000",
                "0x0000000010c6f7a10000000016a09e6600000000239791f10000000000000000",
                "0x00182bc20002505d0002505b0000000000000000000000000000000000000000",
            ],
            "n_layers": 28, "kvd": 1024, "dim": 1024, "vocab": 151936,
            "stop0": 151645, "stop1": 151643, "seq_cap": 1024,
            "stages": 4, "argmax_shards": 2,
        },
        "max_prompt": 992, "max_new_cap": 24,
    },
    "qwen35": {
        "consumer": "0xfd0EF988216D0346BF115530387021c1b699336d",
        "req": {
            "consumer": "0xfd0EF988216D0346BF115530387021c1b699336d",
            "seg_engine": "0xcA459C95ee034D21339cd5ad7209441fD54bcd51",
            "weights_root": "0x0000000000000000000000000000000000000000",
            "manifest": "0x7bdf4876a6861287521dadab3d3870f74dfa557507ed200d49f75bcb09f01fa9",
            "packed_config": [
                "0x0800020028100201000003ca0000400101004004080100020000000000000000",
                "0x0000000010c6f7a10000000010000000000000081527a02b0000000000000000",
                "0x002b48c60003c8ee0003c8ec2010008000800400000000000000000000000000",
            ],
            "packed_config_w3": "0x0000000016a09e66000000000000000000000000000000000000000000000000",
            "n_layers": 40, "kvd": 512, "dim": 2048, "vocab": 248320,
            "stop0": 248046, "stop1": 248044, "seq_cap": 64,
            "model": "qwen35", "stages": 4, "argmax_shards": 2,
        },
        "max_prompt": 56, "max_new_cap": 8,
    },
}

ASKS = {}  # ask_id -> status dict
LOCK = threading.Lock()


def http_json(url, payload=None, headers=None, timeout=30):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(url, data=data, method="POST" if data else "GET")
    # public RPCs and the ingress WAF 403 the default Python-urllib UA
    req.add_header("User-Agent", "gk-shard-bridge/1")
    req.add_header("Content-Type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


# The bridge only needs eth_call (stateTransitionCount) + eth_blockNumber, which
# every public Sepolia endpoint serves. publicnode's free tier intermittently
# 403s the cluster's shared egress IP, so try several endpoints with a retry
# each — drpc first (measured reliable for these calls), configured RPC next.
RPC_ENDPOINTS = []
for _u in ("https://sepolia.drpc.org", RPC, "https://sepolia.gateway.tenderly.co",
           "https://ethereum-sepolia-rpc.publicnode.com"):
    if _u and _u not in RPC_ENDPOINTS:
        RPC_ENDPOINTS.append(_u)


def rpc_call(method, params):
    payload = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
    last = None
    for url in RPC_ENDPOINTS:
        for _ in range(2):
            try:
                j = http_json(url, payload, timeout=30)
                if "result" in j and j["result"] is not None:
                    return j["result"]
                last = RuntimeError(str(j.get("error", j)))
            except Exception as e:  # noqa: BLE001 - try the next endpoint
                last = e
    raise last or RuntimeError("all RPC endpoints failed")


def word(v: int) -> str:
    return format(v, "064x")


def encode_fulfil(prompt_ids, max_new, answer_ids, root_hex) -> bytes:
    # fulfil(uint32[] promptIds, uint256 maxNewTokens, uint32[] answerIds, bytes32 pipelineRoot)
    root = root_hex[2:] if root_hex.startswith("0x") else root_hex
    off_a = 4 * 32
    off_c = off_a + 32 * (1 + len(prompt_ids))
    head = word(off_a) + word(max_new) + word(off_c) + root
    tail_a = word(len(prompt_ids)) + "".join(word(t) for t in prompt_ids)
    tail_c = word(len(answer_ids)) + "".join(word(t) for t in answer_ids)
    return bytes.fromhex(SEL_FULFIL + head + tail_a + tail_c)


def infer_progress(model):
    """Best-effort live progress for an in-flight inference: match the shard
    coordinator's /shard/active entry to this ask's model consumer. Never
    raises and never persists — recomputed on every status poll."""
    try:
        consumer = MODELS[model]["consumer"].lower()
        active = http_json(f"{SHARD}/shard/active", timeout=5).get("active", [])
        for e in active:
            if str(e.get("consumer", "")).lower() == consumer:
                return {"segments_done": e.get("segments_done"),
                        "segments_total": e.get("total_segments"),
                        "infer_elapsed_ms": e.get("elapsed_ms")}
    except Exception:  # noqa: BLE001 - progress is best-effort
        pass
    return {}


def run_ask(ask_id, model, prompt_ids, max_new):
    cfg = MODELS[model]
    st = ASKS[ask_id]
    try:
        req = dict(cfg["req"])
        req["prompt_ids"] = prompt_ids
        req["max_new"] = max_new
        st.update(state="inferring", started=time.time())
        resp = http_json(f"{SHARD}/shard/infer", req, timeout=14400)
        answer = resp["answer_ids"]
        root = resp["pipeline_root"]
        st.update(state="settling", answer_ids=answer, pipeline_root=root,
                  infer_seconds=round(time.time() - st["started"], 1))
        # Settlement: one pure-commit round; the operator gate re-verifies the
        # segment commit chain for this pipelineRoot before signing.
        consumer = cfg["consumer"]
        ti = int(rpc_call("eth_call", [{"to": consumer, "data": SEL_COUNT}, "latest"]), 16)
        try:
            height = int(http_json(FORKHEAD)["forkHead"])
        except Exception:  # noqa: BLE001 - proxy down; live head minus a re-fork lag
            height = int(rpc_call("eth_blockNumber", []), 16) - 60
        calldata = encode_fulfil(prompt_ids, max_new, answer, root)
        body = {"body": {
            "target_address": consumer,
            "call_data": list(calldata),
            "transition_index": ti,
            "from_address": FROM_ADDR,
            "value": "0",
            "block_height": height,
        }}
        http_json(f"{API}/trigger", body, headers={"Authorization": f"Bearer {GK_KEY}"}, timeout=60)
        st.update(state="done", settle_submitted=True, transition_index=ti,
                  total_seconds=round(time.time() - st["started"], 1))
    except Exception as e:  # noqa: BLE001 - surfaced to the poller
        st.update(state="error", error=str(e)[:500])


class H(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):  # quiet
        pass

    def do_GET(self):
        if self.path == "/bridge/healthz":
            return self._send(200, {"ok": True, "asks": len(ASKS)})
        if self.path.startswith("/bridge/ask/"):
            ask_id = self.path.rsplit("/", 1)[-1]
            st = ASKS.get(ask_id)
            if not st:
                return self._send(404, {"error": "unknown ask_id"})
            out = dict(st)  # copy: progress is merged per poll, never stored
            if out.get("state") == "inferring":
                out.update(infer_progress(out["model"]))
            return self._send(200, out)
        return self._send(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/bridge/ask":
            return self._send(404, {"error": "not found"})
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
            model = body["model"]
            cfg = MODELS[model]
            prompt_ids = [int(t) for t in body["prompt_ids"]]
            max_new = int(body.get("max_new", 8))
            if not (1 <= max_new <= cfg["max_new_cap"]):
                raise ValueError(f"max_new must be 1..{cfg['max_new_cap']}")
            if not (0 < len(prompt_ids) <= cfg["max_prompt"]):
                raise ValueError(f"prompt length must be 1..{cfg['max_prompt']}")
            if len(prompt_ids) + max_new > cfg["req"]["seq_cap"]:
                raise ValueError(
                    f"prompt ({len(prompt_ids)}) + max_new ({max_new}) exceeds the "
                    f"model's sequence cap ({cfg['req']['seq_cap']})")
            if any(t < 0 or t >= cfg["req"]["vocab"] for t in prompt_ids):
                raise ValueError("token id out of vocab range")
        except Exception as e:  # noqa: BLE001
            return self._send(400, {"error": str(e)[:300]})
        with LOCK:
            # one inference at a time per model keeps the committee predictable
            if any(s["state"] in ("inferring", "settling") and s["model"] == model
                   for s in ASKS.values()):
                return self._send(429, {"error": f"another {model} ask is already running; try later"})
            ask_id = uuid.uuid4().hex[:16]
            ASKS[ask_id] = {"state": "queued", "model": model,
                            "prompt_ids": prompt_ids, "max_new": max_new}
        threading.Thread(target=run_ask, args=(ask_id, model, prompt_ids, max_new),
                         daemon=True).start()
        return self._send(200, {"ask_id": ask_id})


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8090), H).serve_forever()
