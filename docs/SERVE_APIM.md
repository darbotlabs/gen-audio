# Per-node Serve and APIM

gen-audio's service name is `genaid-audio`. A node that hosts it listens on port **8002** with route prefix **`/genaid-audio`**.

This repository does not start Ray Serve, does not ship an APIM policy file, and does not open a socket on import. `gen_audio.serve` only builds the URLs and the health JSON so node config and clients use the same strings.

## Base URL

```text
http://<node>:8002/genaid-audio
```

`<node>` is that machine's address: a DNS name or an IP the caller can route to. Build it with `node_base_url(host)` rather than string-pasting, so the port and prefix stay attached.

```python
from gen_audio.serve import health_payload, health_url, node_base_url

node_base_url("10.1.8.21")
# http://10.1.8.21:8002/genaid-audio

health_url("10.1.8.21")
# http://10.1.8.21:8002/genaid-audio/health

health_payload()
# {"status": "ok", "service": "genaid-audio", "route_prefix": "/genaid-audio"}
```

Pass a host, not a URL. `node_base_url("http://10.1.8.21")` raises.

## Health

Probe the prefix, not the process root:

```text
GET http://<node>:8002/genaid-audio/health
```

A 200 from `http://<node>:8002/health` can belong to a different Serve application on the same port. Treat that as a different check. The body that matches this service is `health_payload()`: `status` of `ok`, `service` of `genaid-audio`, `route_prefix` of `/genaid-audio`.

This scaffold does not define a synthesis HTTP API. Rendering is a local `KokoroOnnxSynthesizer` call (or a WAV you already have). Add a request route on the node when a node deployment exists; do not assume one from this repo.

## Power Table

The Power Table is the inventory of machines that actually run the deployment. APIM backends and clients are filled from rows, one row per process.

Each row has:

| Field | Meaning |
| --- | --- |
| `id` | Stable name in the table |
| `role` | `node` or `shared-gateway` |
| `host` | Bare host, no scheme and no path |
| `port` | 8002 unless a node truly listens elsewhere |
| `base_url` | `http://<host>:<port>/genaid-audio` |
| `health_url` | `base_url + "/health"` |

`power_table_row(node_id, host)` builds a node row. `shared_gateway_row()` builds the shared gateway row.

Example, with the node host still a placeholder:

```json
{
  "service": "genaid-audio",
  "nodes": [
    {
      "id": "<node-id>",
      "role": "node",
      "host": "<node>",
      "port": 8002,
      "base_url": "http://<node>:8002/genaid-audio",
      "health_url": "http://<node>:8002/genaid-audio/health"
    },
    {
      "id": "shared-gateway",
      "role": "shared-gateway",
      "host": "10.1.8.70",
      "port": 8002,
      "base_url": "http://10.1.8.70:8002/genaid-audio",
      "health_url": "http://10.1.8.70:8002/genaid-audio/health"
    }
  ]
}
```

## Do not collapse the table onto 10.1.8.70

`10.1.8.70:8002` is the shared gateway. It is one row, role `shared-gateway`.

* A healthy shared gateway does not mean a given node is healthy. Probe that node's `health_url`.
* APIM should register a backend per node row. The shared gateway backend, if you keep one, is an extra route, not the only origin.
* Clients that can address a node should call `http://<node>:8002/genaid-audio` for that node. Use the shared gateway only when the caller specifically wants that hop.
* Publishing a single APIM operation whose origin is only `10.1.8.70` hides a dead node behind the gateway. The Power Table is what makes the dead node visible.

Suggested client order:

1. An explicit node base URL the caller was given.
2. Otherwise the matching Power Table row (`role` of `node`).
3. The shared gateway row only as an explicit fallback, not as the catalog.

## What a node process is responsible for

On each machine the Serve deployment, outside this scaffold, should:

* bind port 8002
* mount the app at `/genaid-audio`
* serve `GET /genaid-audio/health` with `health_payload()`
* write WAVs and PNGs through `GEN_AUDIO_ARTIFACT_DIR` if the worker disk is scratch (see `docs/ARCHITECTURE.md`)

Copying model weights into the repo or into the image source tree is out of scope here. Point the process at files on disk with `GEN_AUDIO_KOKORO_MODEL` and `GEN_AUDIO_KOKORO_VOICES` when that node runs kokoro-onnx.
