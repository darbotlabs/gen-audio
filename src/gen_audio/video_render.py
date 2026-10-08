"""Render a clip MP4 from the WAV, the 2D spectrogram, and the cube PNG."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path


class VideoError(RuntimeError):
    """ffmpeg could not render the clip. The message names the missing piece."""


def render_clip_mp4(wav: Path, spectrogram_png: Path, cube_png: Path, output: Path) -> Path:
    """Stack the spectrogram, a waveform, and the cube over the clip audio."""
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        raise VideoError("ffmpeg is not on PATH")
    for path in (wav, spectrogram_png, cube_png):
        if not Path(path).is_file():
            raise VideoError(f"video input is missing: {Path(path).name}")
    output.parent.mkdir(parents=True, exist_ok=True)
    filter_graph = (
        "[0:a]showwaves=s=960x180:mode=cline:colors=0xD7E6FF[wave];"
        "[1:v]scale=960:270:force_original_aspect_ratio=decrease,"
        "pad=960:270:(ow-iw)/2:(oh-ih)/2:color=0x10141c[spec];"
        "[2:v]scale=960:270:force_original_aspect_ratio=decrease,"
        "pad=960:270:(ow-iw)/2:(oh-ih)/2:color=0x10141c[cube];"
        "[spec][wave][cube]vstack=inputs=3[v]"
    )
    command = [
        ffmpeg,
        "-y",
        "-i",
        str(wav),
        "-loop",
        "1",
        "-i",
        str(spectrogram_png),
        "-loop",
        "1",
        "-i",
        str(cube_png),
        "-filter_complex",
        filter_graph,
        "-map",
        "[v]",
        "-map",
        "0:a",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        str(output),
    ]
    completed = subprocess.run(command, capture_output=True, text=True, check=False)
    if completed.returncode != 0 or not output.is_file() or output.stat().st_size < 1024:
        tail = (completed.stderr or "")[-500:]
        raise VideoError(f"ffmpeg failed to write {output.name}: {tail}")
    return output
