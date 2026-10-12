"""Bounded RIFF INFO Artist attribution, without changing audio samples."""

from __future__ import annotations

from pathlib import Path
import struct
import unicodedata

from gen_audio.guards import max_audio_file_bytes


def agent_metadata(names) -> dict:
    if not isinstance(names, (list, tuple)) or len(names) > 8:
        raise ValueError("Artist attribution requires at most eight agent names")
    artists = []
    for name in names:
        if (not isinstance(name, str) or not name.strip() or name != name.strip()
                or len(name) > 128 or len(name.encode("utf-8")) > 512
                or any(unicodedata.category(char) == "Cc" for char in name)):
            raise ValueError("Invalid agent Artist name")
        if name not in artists:
            artists.append(name)
    artist = "; ".join(artists)
    if len(artist.encode("utf-8")) > 4095:
        raise ValueError("Combined Artist attribution exceeds its byte limit")
    return {"artist": artist, "artists": artists, "agentNames": artists} if artists else {}


def _chunks(raw: bytes, large_sizes: dict | None = None):
    offset = 0
    while offset < len(raw):
        if offset + 8 > len(raw):
            raise ValueError("Truncated RIFF metadata chunk")
        size = struct.unpack_from("<I", raw, offset + 4)[0]
        if size == 0xFFFFFFFF:
            if large_sizes is None or raw[offset:offset + 4] not in large_sizes:
                raise ValueError("Missing RF64 chunk size")
            size = large_sizes.pop(raw[offset:offset + 4])
        end = offset + 8 + size
        padded = end + (size & 1)
        if padded > len(raw):
            raise ValueError("RIFF metadata chunk exceeds its container")
        yield raw[offset:offset + 4], raw[offset + 8:end], raw[offset:padded]
        offset = padded


def _wave(raw: bytes):
    if raw[:4] == b"RF64":
        if len(raw) < 48 or raw[8:16] != b"WAVEds64":
            raise ValueError("RF64 requires a leading ds64 chunk")
        ds64_size = struct.unpack_from("<I", raw, 16)[0]
        riff_size, data_size, _samples, count = struct.unpack_from("<QQQI", raw, 20)
        if (raw[4:8] != b"\xff\xff\xff\xff" or ds64_size < 28 or count > 1024
                or ds64_size < 28 + count * 12 or 20 + ds64_size > len(raw)
                or riff_size + 8 != len(raw)):
            raise ValueError("Invalid RF64 size table")
        sizes = {b"data": data_size}
        for offset in range(48, 48 + count * 12, 12):
            tag = raw[offset:offset+4]
            if tag in sizes:
                raise ValueError("Ambiguous RF64 size table")
            sizes[tag] = struct.unpack_from("<Q", raw, offset+4)[0]
        return _chunks(raw[12:], sizes)
    if (len(raw) < 12 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE"
            or struct.unpack_from("<I", raw, 4)[0] + 8 != len(raw)):
        raise ValueError("Artist metadata requires a complete RIFF WAV")
    return _chunks(raw[12:])


def read_artist(path: Path | str) -> str | None:
    path = Path(path)
    if path.stat().st_size > max_audio_file_bytes():
        raise ValueError("Audio metadata input exceeds the byte limit")
    artists = []
    for kind, data, _ in _wave(path.read_bytes()):
        if kind == b"LIST" and data[:4] == b"INFO":
            for tag, value, _ in _chunks(data[4:]):
                if tag == b"IART":
                    if len(value) > 4096 or not value.endswith(b"\0"):
                        raise ValueError("Invalid WAV Artist metadata")
                    artists.append(value[:-1].decode("utf-8"))
    if len(artists) > 1:
        raise ValueError("Ambiguous WAV Artist metadata")
    return artists[0] if artists else None


def tag_wav(path: Path | str, names) -> dict:
    """Tag a private staged WAV before publication/hashing; preserve other chunks."""
    metadata = agent_metadata(names)
    if not metadata:
        return metadata
    path = Path(path)
    if path.stat().st_size > max_audio_file_bytes():
        raise ValueError("Audio metadata input exceeds the byte limit")
    raw = path.read_bytes()
    if raw[:4] != b"RIFF":
        raise ValueError("Tag a staged RIFF WAV before publishing; preserve RF64 Artist when re-encoding")
    parts = []
    for kind, data, encoded in _wave(raw):
        if kind == b"LIST" and data[:4] == b"INFO":
            other = b"".join(chunk for tag, _, chunk in _chunks(data[4:]) if tag != b"IART")
            if other:
                data = b"INFO" + other
                parts.append(b"LIST" + struct.pack("<I", len(data)) + data)
        else:
            parts.append(encoded)
    value = metadata["artist"].encode("utf-8") + b"\0"
    info = b"INFOIART" + struct.pack("<I", len(value)) + value + b"\0" * (len(value) & 1)
    parts.append(b"LIST" + struct.pack("<I", len(info)) + info)
    body = b"WAVE" + b"".join(parts)
    result = b"RIFF" + struct.pack("<I", len(body)) + body
    if len(result) > max_audio_file_bytes():
        raise ValueError("Tagged WAV exceeds the byte limit")
    path.write_bytes(result)
    if read_artist(path) != metadata["artist"]:
        raise ValueError("WAV Artist attribution failed verification")
    return metadata
