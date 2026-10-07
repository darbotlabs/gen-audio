"""URL helpers for a Gen-Audio node.

Importing this module does not start a server. ``GET /genaid-audio/health`` is
liveness. ``GET /genaid-audio/ready`` is readiness. See ``docs/SERVE_APIM.md``.
"""

from __future__ import annotations

SERVICE_NAME = "genaid-audio"
DEFAULT_PORT = 8002
ROUTE_PREFIX = "/genaid-audio"
# Shared gateway address. It is one Power Table row, not a substitute for
# each node's own base URL.
SHARED_GATEWAY_HOST = "10.1.8.70"


def node_base_url(host: str, port: int = DEFAULT_PORT, route_prefix: str = ROUTE_PREFIX) -> str:
    """Build ``http://<host>:<port>/genaid-audio``."""
    bare = _bare_host(host)
    listen = _port(port)
    prefix = _prefix(route_prefix)
    return f"http://{bare}:{listen}{prefix}"


def health_url(host: str, port: int = DEFAULT_PORT, route_prefix: str = ROUTE_PREFIX) -> str:
    """Health URL under the service prefix, not the process root."""
    return node_base_url(host, port, route_prefix) + "/health"


def ready_url(host: str, port: int = DEFAULT_PORT, route_prefix: str = ROUTE_PREFIX) -> str:
    """Readiness URL. Liveness stays on ``/health`` when the model is unloaded."""
    return node_base_url(host, port, route_prefix) + "/ready"


def health_payload() -> dict[str, str]:
    """Liveness body. This stays successful when the model is not loaded."""
    return {
        "status": "ok",
        "service": SERVICE_NAME,
        "route_prefix": ROUTE_PREFIX,
    }


def ready_payload(*, model_loaded: bool = False) -> dict[str, str | bool]:
    """Readiness body. An unloaded model is still ready because load is lazy."""
    return {
        "ready": True,
        "status": "ready",
        "service": SERVICE_NAME,
        "route_prefix": ROUTE_PREFIX,
        "model_loaded": model_loaded,
        "speech_generation": "available" if model_loaded else "load_required",
        "script_generation": "available",
    }


def power_table_row(
    node_id: str,
    host: str,
    *,
    role: str = "node",
    port: int = DEFAULT_PORT,
) -> dict[str, str | int]:
    """One Power Table row: the inventory record APIM uses for a backend."""
    label = node_id.strip()
    if not label:
        raise ValueError("node id is empty")
    duty = role.strip()
    if not duty:
        raise ValueError("role is empty")
    base = node_base_url(host, port)
    return {
        "id": label,
        "role": duty,
        "host": _bare_host(host),
        "port": _port(port),
        "base_url": base,
        "health_url": base + "/health",
    }


def shared_gateway_row(port: int = DEFAULT_PORT) -> dict[str, str | int]:
    """The shared gateway as its own row."""
    return power_table_row("shared-gateway", SHARED_GATEWAY_HOST, role="shared-gateway", port=port)


def _bare_host(host: str) -> str:
    value = host.strip()
    if not value:
        raise ValueError("host is empty")
    if "://" in value or "/" in value:
        raise ValueError(
            "pass a host such as 10.1.8.21, not a URL; the helper builds "
            "http://<host>:8002/genaid-audio"
        )
    if ":" in value and not (value.startswith("[") and value.endswith("]")):
        raise ValueError("pass the port as its own argument, not inside the host")
    return value


def _port(port: int) -> int:
    listen = int(port)
    if listen < 1 or listen > 65535:
        raise ValueError(f"port must be between 1 and 65535, got {port}")
    return listen


def _prefix(route_prefix: str) -> str:
    text = route_prefix.strip()
    if not text.startswith("/"):
        text = "/" + text
    return text.rstrip("/") or ""
