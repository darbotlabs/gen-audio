"""Errors for voice documents, catalogs, and framework adapters."""


class VoiceError(ValueError):
    """A voice, G2P config, catalog, or cast reference failed validation.

    ``code`` is the stable token shared with the Rust checker and the invalid
    fixtures under ``tests/fixtures/voices/invalid``.
    """

    def __init__(self, message: str, code: str = "invalid"):
        self.code = code
        super().__init__(f"{code}: {message}" if not str(message).startswith(f"{code}:") else message)
