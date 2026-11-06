"""Voice descriptors, framework adapters, and the voice-library catalog.

Importing this package does not load a TTS runtime or a model weight.
"""

from gen_audio.voices.blend import blend_from_pack, blend_vectors, load_style_pack
from gen_audio.voices.builtins import mock_offline_request
from gen_audio.voices.catalog import LIBRARY_ENV, Catalog, library_root, load_catalog, missing_assets
from gen_audio.voices.errors import VoiceError
from gen_audio.voices.model import G2pConfig, VoiceDescriptor, load_g2p, load_voice, validate_g2p, validate_voice
from gen_audio.voices.plan import SynthesisPlan
from gen_audio.voices.registry import FrameworkSpec, get_framework, list_frameworks, register, unregister
from gen_audio.voices.resolve import validate_cast

__all__ = [
    "LIBRARY_ENV",
    "Catalog",
    "FrameworkSpec",
    "G2pConfig",
    "SynthesisPlan",
    "VoiceDescriptor",
    "VoiceError",
    "blend_from_pack",
    "blend_vectors",
    "get_framework",
    "library_root",
    "list_frameworks",
    "load_catalog",
    "load_g2p",
    "load_style_pack",
    "load_voice",
    "missing_assets",
    "mock_offline_request",
    "register",
    "unregister",
    "validate_cast",
    "validate_g2p",
    "validate_voice",
]
