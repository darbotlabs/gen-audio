from __future__ import annotations

import numpy as np

from gen_audio.cube_revision import (
    decide_ops,
    expand_crest,
    lift_body,
    measure,
    reduce_clip,
    revise,
    write_cube_plot,
)


def test_reduce_clip_on_full_scale_constant():
    audio = np.ones(4_000, dtype=np.float32)
    result = revise(audio, 16_000)
    assert "reduce_clip" in result.ops
    assert result.after.clip_frac == 0.0
    assert result.after.peak <= 0.89 + 1e-5
    assert result.before.inv_hdr == 1.0


def test_expand_crest_lowers_inv_hdr_when_body_is_dense():
    pattern = np.array([0.9, -0.9, 0.9, 0.2, -0.2], dtype=np.float64)
    audio = np.tile(pattern, 400)
    before = measure(audio, 16_000)
    assert before.inv_hdr > 0.45
    expanded = expand_crest(audio)
    after = measure(expanded, 16_000)
    assert after.inv_hdr < before.inv_hdr
    assert "expand_crest" in decide_ops(before)


def test_lift_body_raises_inv_hdr_under_a_spike():
    sample_rate = 16_000
    time = np.arange(sample_rate) / sample_rate
    audio = (0.02 * np.sin(2 * np.pi * 220 * time)).astype(np.float64)
    audio[100] = 1.0
    before = measure(audio, sample_rate)
    assert before.inv_hdr < 0.08
    lifted = lift_body(audio)
    after = measure(lifted, sample_rate)
    assert after.inv_hdr > before.inv_hdr
    assert "lift_body" in decide_ops(before)


def test_clipped_peak_scales_to_target():
    audio = np.array([1.0, -1.0, 0.2], dtype=np.float64)
    limited = reduce_clip(audio, 0.89)
    assert np.isclose(float(np.max(np.abs(limited))), 0.89)


def test_cube_plot_writes_png(tmp_path):
    audio = np.ones(2_000, dtype=np.float32)
    result = revise(audio, 16_000, max_steps=1)
    destination = tmp_path / "cube.png"
    write_cube_plot(destination, result.before, result.after)
    assert destination.stat().st_size > 1000
