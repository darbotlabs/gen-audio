import pytest
from gen_audio.node_http import serve_loopback

@pytest.mark.parametrize("host", ["0.0.0.0", "192.0.2.1", "::", "example.com"])
def test_loopback_helper_rejects_non_loopback_before_binding(host):
    with pytest.raises(ValueError, match="loopback"):
        serve_loopback(host, 0)

def test_loopback_helper_accepts_ephemeral_local_listener():
    server = serve_loopback("127.0.0.1", 0)
    try:
        assert server.server_address[0] == "127.0.0.1"
        assert server.server_address[1] > 0
    finally:
        server.server_close()
