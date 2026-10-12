# BITDOT_BRAILLE_ANALYSIS.md: "bitdot braille" VibeVoice podcast, cube and 2D channels

Generated: 2026-10-07, about 21:15 PT, on the box (CPU only), `/workspace/genaid-podcast-compare/`
Clip: `audio/bitdot_braille_vibevoice.wav`. Real VibeVoice-1.5B output: 24 kHz, mono, PCM16, **247.067 s** (5,929,600 frames).
Every number below is measured from on-disk files by the scripts listed in section 9. Where something is interpretation rather than measurement, it says so.

---

## 0. Provenance (is it real?)

| Item | Value |
|---|---|
| Engine | microsoft/VibeVoice-1.5B, weights `models/VibeVoice-1.5B`, code `models/VibeVoice-community/demo/inference_from_file.py` @ `952326d` |
| Voices | Speaker 1 = Alice (`demo/voices/en-Alice_woman.wav`), Speaker 2 = Frank (`demo/voices/en-Frank_man.wav`); same cast as `voices/cast_map.json` and `genaid_full_vibevoice_*` |
| Run | one pass, no chunking; CPU; float16; cfg 1.3; seed 42; 26 segments |
| Wall time | **1478.2 s** (24.6 min, includes model load). VibeVoice reported generation 1434.29 s, RTF 5.81x, 1306 prefill and 1867 generated tokens, EOS at step 1867 of 2612 |
| ASR round trip | Nemotron 3.5 ASR streaming 0.6B (q8_0, NeMo-Speech.cpp CPU) against `podcast_bitdot_braille.txt`: **WER 2.89 %, CER 1.44 %** (761 ref words: 741 hits, 13 sub, 7 del, 2 ins) |
| Metadata | `audio/bitdot_braille_vibevoice.synth.json`, `transcripts/bitdot_braille_wer.json`, log `logs/vv_bitdot_braille.log` |

The ASR errors are mostly coined words: "bitdot braille" came back as "bitbraille", "bitdot cube" as "bitcube", and "kilohertz" as "khz". There is also one real slip, "lifts the quiet parts" heard as "lifts apart". For comparison, the earlier `genaid_full_vibevoice_final` scored 14.60 % WER against its own script (S15_STATUS.md).

## 1. Method (unchanged from CUBE_ANALYSIS.md)

- STFT: n_fft 1024, hop 256 (10.67 ms), Hann window, 24 kHz. That gives 23,163 frames x 513 bins.
- Layers from `scripts/cube_spectrogram_3d.py`, code unchanged:
  - **signal**: percentile-stretched log magnitude with a ^0.65 shadow lift (the inverse-HDR part)
  - **tonality**: 1 − spectral flatness, mixed with local peakiness
  - **confidence**: log SNR against a per-row 20th-percentile noise floor
  - **quality**: midband/HF-clarity/mud proxy
- Cube: block-average to at most 128 x 400 (here sf 5, st 58, so **102 freq x 399 time bins, 0.6187 s per bin**, covering 246.85 of 247.07 s). Points are blocks ≥ **0.12**.
- inv_hdr = 0.35·signal + 0.25·tonality + 0.20·confidence + 0.20·quality, using full-resolution means.
- Library JSON: `scripts/library_cube_generic.py`, the same algorithm as `library_cube_rev2.py`. It keeps 900 top points per layer, normalizes t and f, sets z = layer/3, and min-max scales v per layer. **cube_revision 1**, full duration.
- Reproducibility check: the same code run on `genaid_full_vibevoice_final.wav` gives inv_hdr 0.2162 with 101,652 points. That matches CUBE_ANALYSIS.md exactly. On `genaid_full_misaki_kokoro.wav` (139.375 s) it gives 0.23948, which matches the rev2 library JSON.

## 2. Headline numbers vs. the two reference clips

All three clips were re-measured here with the same code.

| Metric | **bitdot_braille_vibevoice** | genaid_full_vibevoice_final | genaid_full_misaki_kokoro |
|---|---:|---:|---:|
| Duration (s) | **247.067** | 117.045 | 139.375 |
| Peak / RMS dBFS | 0.744 / −26.08 | 0.890 / −20.57 | 0.659 / −26.70 |
| **inv_hdr** | **0.1869** | 0.2162 | 0.2395 |
| inv_hdr formula on speech frames only (signal occupancy ≥ 0.05) | 0.2237 | 0.2351 | 0.2982 |
| Speech-frame fraction | 0.750 | 0.876 | 0.747 |
| Cube shape (f x t), s per bin | 102 x 399, 0.619 | 102 x 391, 0.299 | 102 x 395, 0.352 |
| Cube points, total | 94,667 | 101,652 | 115,689 |
| signal points | 12,077 | 15,101 | 14,317 |
| tonality points | 18,766 | 21,848 | 27,416 |
| confidence points | 23,126 | 24,821 | 33,666 |
| quality points | 40,698 | 39,882 | 40,290 |
| Layer mean (full res): signal / tonality / confidence / quality | 0.089 / 0.161 / 0.278 / 0.301 | 0.118 / 0.210 / 0.301 / 0.310 | 0.111 / 0.212 / 0.454 / 0.284 |
| Layer p90: signal / tonality / confidence / quality | 0.271 / 0.577 / 1.000 / 0.396 | 0.357 / 0.590 / 0.978 / 0.412 | 0.293 / 0.577 / 1.000 / 0.407 |
| Occupancy ≥ 0.12 (full res): s / t / c / q | 0.214 / 0.277 / 0.351 / 0.991 | 0.283 / 0.366 / 0.458 / 0.996 | 0.283 / 0.388 / 0.503 / 0.977 |
| BW90 / **BW95** / BW99 (Hz) | 1723 / **2250** / 3832 | 1969 / 2777 / 4137 | 1336 / 3398 / 9234 |
| Energy fraction > 4 kHz / > 8 kHz | 0.80 % / 0.0003 % | 1.23 % / 0.0013 % | 4.37 % / 1.61 % |
| Highest frequency within 60 dB of PSD peak | 9176 Hz | 11297 Hz | 12000 Hz |
| Braille cells (1 per s): flat / distinct patterns | 23.5 % / 23 | 18.8 % / 26 | 18.7 % / 22 |

BW95 here is the frequency below which 95 % of the Welch power (nperseg 2048) lies. COMPARISON.md lists 2670.5 Hz for vibevoice_final, from an implementation that is no longer on disk. With one method applied to all three clips, the comparison above is internally consistent.

**Readings (measured):**
- inv_hdr for the new episode is **0.0293 lower** than vibevoice_final. With silent frames excluded, the gap shrinks to **0.0114** (0.2237 vs 0.2351). So most of the headline gap comes with the larger share of pause frames (25.0 % vs 12.4 % of frames have signal occupancy < 0.05). vibevoice_final went through the improve pipeline, which trims dead air. This clip is raw.
- It is also **narrower-band** than vibevoice_final: BW95 2250 vs 2777 Hz, and BW99 3832 vs 4137 Hz. Essentially nothing lies above 8 kHz (3.4e-6 of the power). The linear spectrogram shows a hard ceiling near 7.7–8 kHz, and the confidence and tonality channel maps end at the same line. Misaki/Kokoro reaches the 12 kHz Nyquist limit.
- The quality layer is **saturated at the cube threshold**: 99.1 % occupancy, and 99.94 % even in RMS-silent frames, because the formula has a per-frame floor. That is why quality has the most points in every clip (about 40k each) but almost no structure in point count.

## 3. Energy over time

- 1-second RMS: p10 −31.6, p90 −23.6 dBFS. The loudest second is 0 s (−21.5 dBFS) and the quietest is 241 s (−40.6 dBFS, the pause between turns 24 and 25).
- 10-second windows range from −29.9 dBFS (210–220 s) to −25.5 dBFS (180–190 s).
- First half −26.84, second half −27.42 dBFS. The linear trend is −0.34 dB/min, so the level holds steady across the 4-minute single pass with no late collapse.
- `bitdot_braille_vibevoice_timeline.png` plots the 1-second RMS and per-layer occupancy, with dashed lines at the ASR-aligned turn starts. The deep RMS notches sit on the turn changes.

## 4. Speaker turns in the layers

The 26 script turns were aligned to the ASR word timestamps (741 of 761 words matched). Pitch (F0) was estimated with librosa YIN on voiced frames.

| | Alice (Speaker 1), 13 turns | Frank (Speaker 2), 13 turns |
|---|---|---|
| Median F0 per turn | median 196 Hz (172–217) | median 115 Hz (103–123) |
| Signal occupancy per turn | 0.147–0.196 | 0.197–0.338 |
| Mean signal value per turn | 0.081 (0.068–0.092) | 0.107 (0.095–0.146) |
| Mean tonality value per turn | 0.149 (0.125–0.162) | 0.190 (0.169–0.237) |
| Mean confidence value per turn | 0.259 (0.216–0.283) | 0.329 (0.291–0.423) |
| Mean quality value per turn | 0.315 (0.295–0.330) | 0.283 (0.243–0.301) |

- **The turns show up as an alternating step.** Every Frank turn has higher signal occupancy and a higher mean signal than every Alice turn; the per-turn ranges don't overlap. Tonality and confidence follow the same pattern. Quality runs the other way: Alice is higher. F0 separates the two voices cleanly in all 26 turns.
- **Inter-turn pauses.** Time-domain RMS silence (frames more than 40 dB below the loudest frame) turns up a silent run at all 25 turn changes: median 0.65 s, range 0.34–1.02 s, 22 of them at least 0.5 s. Signal-layer occupancy averages **0.0068** in RMS-silent frames and **0.266** in the rest. Tonality is 0.026 vs 0.341 and confidence 0.072 vs 0.422. These gaps are the vertical dark columns in the spectrogram and the white columns in the channel maps.
- Interpretation: Frank's higher signal, tonality and confidence fit a lower-F0 voice with closely spaced harmonics, which put more bins above threshold. His lower quality fits the quality formula's penalty on energy at or below 200 Hz ("mud"), where his fundamental sits. This follows from how the formula is built. It isn't a separate measurement of perceived quality.

## 5. Where confidence and quality dip

These are speech-active seconds only (signal occupancy at or above the clip median), lowest six per layer.

| Layer | Second | Mean | Active median | Turn | ASR words in that second |
|---|---:|---:|---:|---|---|
| confidence | 33 | 0.268 | 0.348 | 3 (Alice) | "gives a grid time" |
| confidence | 121 | 0.271 | | 12 (Frank) | "clip, busier than" |
| confidence | 128 | 0.265 | | 13 (Alice) | "is our interpretation." (RMS −33.0 dBFS, end of phrase) |
| confidence | 176 | 0.280 | | 18 (Frank) | "like a clip, a" |
| confidence | 221 | 0.280 | | 22 (Frank) | "from measuring sound." |
| confidence | 238 | 0.282 | | 24 (Frank) | "don't, we'll say" |
| quality | 11 | 0.231 | 0.296 | 2 (Frank) | "bitcube that we build from" |
| quality | 17 | 0.252 | | 2 (Frank) | "starts to read like Braille" |
| quality | 66 | 0.252 | | 6 (Frank) | "with a penalty for low" |
| quality | 90 | 0.247 | | 8 (Frank) | "time and that frequency." |
| quality | 163 | 0.253 | | 16 (Frank) | "hints at a muddy or band" |
| quality | 246 | 0.249 | | 26 (Frank) | "See you next time." |

All six quality dips fall in Frank turns, which matches the per-speaker quality gap in section 4. The confidence dips are split 2 Alice / 4 Frank and sit at phrase ends or quieter stretches. None of the dips lines up with an ASR error. The worst ASR miss, "lifts the quiet parts" (heard as "lifts apart"), lands at 41.6–42.7 s, which isn't on either list.

## 6. 2D spectrogram channel visuals

| File | What it shows |
|---|---|
| `bitdot_braille_vibevoice_spectrogram.png` | Full 247 s. Top: linear STFT dB (0–12 kHz). Bottom: 128-band mel dB. Turn starts are marked. Harmonic stacks run up to about 4 kHz, a weak band continues to about 7.7 kHz, and above that is black. |
| `bitdot_braille_vibevoice_channel_{signal,tonality,confidence,quality}.png` | Each full-resolution layer as a 2D map (value 0–1, layer colour). |
| `bitdot_braille_vibevoice_channels_panel.png` | All four stacked: quality, confidence, tonality, signal from top to bottom, in cube order. |
| `bitdot_braille_vibevoice_strip.png` + `_strip.json` | Livetile strip, 5930 x 64 px, 64 mel bands from 0–12 kHz with the low band at the bottom, magma colours, dB re max clipped to [−80, 0]. **x = t · 24.0016 px/s**; s_per_px = 0.041664; about 3.906 STFT frames per pixel. |
| `bitdot_braille_vibevoice_timeline.png` | 1-second RMS plus per-layer occupancy with turn markers. |

Comparing the cube layers with the 2D channels:
- The **signal** channel is the only one with visible formant shape. It is densest below about 1 kHz and fades by about 4 kHz. In the cube, it is the sparsest layer (12,077 points).
- **Tonality** and **confidence** are almost binary in 2D. Their p90 values are 0.577 and 1.000, and their p50 is 0. They switch on as full-height columns up to the roughly 7.7 kHz ceiling whenever there is speech. In the cube, that shows up as more points (18,766 and 23,126) spread over most of the frequency axis.
- **Quality** is a near-uniform wash in 2D, with a mean of 0.301 and a p90−p50 spread of only 0.10. The flat pink plane at the top of the cube is that wash, and it is the reason the quality row of the braille reading needs special handling (section 7).
- The cube's 0.62 s time bins average away the inter-turn pauses (0.34–1.02 s). The 2D channels at 10.7 ms per frame keep every one of them. For pause and turn analysis, use the 2D channels or the strip. The cube is the better overview of how dense each layer is.

## 7. Braille reading (interpretation, with a measured basis)

**Method.** Each one-second slice becomes one 2x4 cell:
- **Columns**: the left column is the first half-second and the right column is the second. Windows are exact time intervals [k·0.5, (k+1)·0.5) s.
- **Rows, top to bottom**: quality, confidence, tonality, signal. This matches the cube stack, with signal at the bottom.
- **Occupancy**: the fraction of the 513 STFT bins whose layer value is ≥ 0.12, the cube threshold.
- **Raised dot**: when that half-second's occupancy is above the layer's **median** half-second occupancy over the whole clip. The medians are signal 0.2254, tonality 0.2924, confidence 0.3771 and quality 1.0000.
- **Unicode**: the left column is dots 1, 2, 3, 7 and the right column is dots 4, 5, 6, 8. The code point is U+2800 + Σ bits.

*Caveat (measured):* the quality median is 1.0, so under pure occupancy the **top row can never rise**, and no cell ever has all 8 dots. Variant Q below raises the quality dot when the half-second **mean quality value** is above its clip median, 0.2995, and keeps the other three rows unchanged.

**Test of the episode's own claim** that pauses read as flat cells:
- Of the 18 half-seconds that sit at least 80 % inside an inter-turn RMS silence, **18 of 18 (100 %)** have signal, tonality and confidence all flat. Their mean signal occupancy is 0.0079.
- In half-seconds with almost no silence (< 5 % silent frames, n = 247), 24.3 % have a flat signal dot and 16.2 % have all three flat.
- **All 25 of 25** inter-turn silences contain at least one half-second with those three dots flat. So the claim holds on this clip.

**Six slices** (rendered from the real data; ● raised, ○ flat):

| Slice | Context (ASR words, RMS) | Occupancy cell | Rows, top to bottom (q, c, t, s), left \| right | Occupancy, left → right (s / t / c) | Variant Q |
|---|---|---|---|---|---|
| 3–4 s | Alice, "Frank today's", −27.5 dBFS | ⢰ U+28B0 | ○○ / ○● / ○● / ○● | s 0.004 → 0.296; t 0.014 → 0.398; c 0.052 → 0.502 | ⢹ U+28B9 |
| 6–7 s | end of turn 1 ("bitbraille."), then the pause | ⠀ U+2800 | all ○ | s 0.224 → 0.014 (left is 0.0014 under the median: an edge case) | ⠀ U+2800 |
| 10–11 s | Frank, "inverse HDR bitcube", −24.0 dBFS | ⣶ U+28F6 | ○○ / ●● / ●● / ●● | s 0.320 → 0.273; t 0.376 → 0.343; c 0.442 → 0.419 | ⣶ U+28F6 |
| 18–19 s | inter-turn silence 17.92–18.62 s, then Alice's "Before" | ⠀ U+2800 | all ○ | s 0.000 → 0.118; t 0.000 → 0.193; c 0.003 → 0.277 | ⠈ U+2808 |
| 128–129 s | Alice, "is our interpretation.", −33.0 dBFS | ⢠ U+28A0 | ○○ / ○○ / ○● / ○● | s 0.185 → 0.241; t 0.251 → 0.299; c 0.326 → 0.361 | ⢩ U+28A9 |
| 196–197 s | silence 195.57–196.60 s, then Frank's "Which makes" | ⢰ U+28B0 | ○○ / ○● / ○● / ○● | s 0.017 → 0.303; t 0.069 → 0.371; c 0.149 → 0.437 | ⢹ U+28B9 |

The pattern ⢰ (left column flat, right column raised) marks speech onset after a pause; it shows up at both 3 s and 196 s. The pattern ⣶ (six dots, top row flat) is the steady voiced-phrase cell. Of the 66 ⣶ cells, 55 fall in Frank's turns and 9 in Alice's (2 at boundaries), even though Frank's turns take up 125 s against Alice's 98 s.

**Full line**, one cell per second, pure occupancy (247 cells):
```
   0s  ⣶⣶⠀⢰⢰⠀⠀⢠⣶⣶⣶⢴⣶⢰⢰⡆⣶⡆⠀⣶⠐⢰⠀⣶⠂⡆⡆⣠⠆⠀⠀⡆⠀⠀⠀⠀⣦⣶⢰⣶⢰⣶⠀⢰⣶⢰⣶⠀⡆⠀⡆⠆⡶⡆⠆⠀⢰⣶⣶⣶
  60s  ⡀⣶⣶⣶⣶⠀⣶⠀⢰⡆⠀⠀⢀⣀⠀⣶⠀⠀⠀⠂⠀⢰⢀⠀⢀⡆⣀⢰⣶⣶⢰⠀⡆⠀⢰⣶⡆⢠⡆⣶⠀⣶⡆⣶⡆⢰⣶⡖⢰⡆⢰⡆⠀⠀⢰⢰⣶⣶⢰⣶
 120s  ⡆⢰⢰⢰⢰⠀⣶⣶⢠⢰⡆⡆⠀⣴⢲⠀⠐⡆⠀⠀⣆⣶⣶⣶⢰⡆⣶⡆⠐⣶⠀⢰⣶⡆⣶⡆⣶⣶⣶⡀⣶⣦⢰⣶⡆⣶⣶⡆⠀⣶⢶⠀⠆⠀⠰⣶⢠⣰⢰⣶
 180s  ⢰⢰⢰⠀⣶⣶⣶⡄⢰⠆⠀⠐⠆⠐⠀⠀⢰⣶⠀⣶⡶⡆⢰⢰⣶⢰⡆⡆⣶⣶⠀⡶⡆⠀⠀⠆⠀⠀⢰⣶⢰⡆⣶⣶⡆⢰⣶⠆⡆⠀⠀⠀⠂⠀⠀⢀⠀⢰⢰⢰
 240s  ⣶⠀⣆⢰⠀⠀⣶
```
**Variant Q** (quality row uses the mean value):
```
   0s  ⣾⣾⠉⢹⢹⠈⠀⢨⣷⣶⣶⢴⣶⢸⢱⡎⣾⡆⠈⣿⠙⢹⠉⣿⠋⡏⡇⣡⠏⠉⠉⡏⠉⠁⠉⠁⣦⣾⢹⣶⢰⣾⠀⢱⣷⢹⣶⠉⡏⠉⡇⠏⡿⡇⠆⠉⢰⣶⣶⣶
  60s  ⡈⣶⣶⣶⣶⠀⣶⠉⢹⡆⠁⠁⢁⣈⠈⣷⠁⠀⠉⠋⠉⢸⢀⠁⢀⡆⣀⢰⣾⣶⢰⠉⡇⠁⢰⣿⡆⢡⡏⣾⠁⣿⡆⣿⡆⢹⣶⡞⢰⡎⢹⡆⠀⠈⢱⢱⣾⣶⢱⣷
 120s  ⡆⢰⢰⢰⢸⠉⣿⣾⢩⢹⡇⡇⠀⣽⢻⠀⠙⡏⠈⠈⣆⣶⣾⣶⢰⡆⣶⡏⠙⣿⠈⢱⣶⡆⣷⡇⣾⣿⣶⡀⣾⣧⢱⣶⡇⣿⣿⡎⠉⣿⢾⠉⠇⠉⠱⣾⢠⣰⢱⣶
 180s  ⢱⢹⢰⠈⣾⣶⣷⡄⢹⠏⠉⠙⠇⠙⠀⠈⢹⣶⠈⣾⡾⡏⢰⢰⣷⢱⡇⡎⣷⣿⠉⡿⡇⠈⠉⠇⠁⠁⢱⣶⢰⡇⣶⣷⡆⢹⣿⠏⡇⠁⠁⠉⠋⠉⠁⢈⠉⢱⢸⢰
 240s  ⣶⠉⣇⢹⠁⠀⣶
```
Under pure occupancy, the three clips share the same mean of 3.0 raised dots per cell; that is close to built in, because every raise threshold is a median. Flat-cell fractions are 23.5 % (bitdot), 18.8 % (vibevoice_final) and 18.7 % (misaki). The bitdot clip's higher flat fraction matches its longer total pause time.

The Gen-Audio "schema glyph" (an asset UID drawn as a 2x4 Unicode braille cell) is only discussed in the podcast script. No glyph implementation existed in the box's gen-audio checkouts when this was written, so none is analyzed here.

## 8. Summary

1. Real VibeVoice-1.5B two-speaker audio: 247.07 s, ASR WER 2.89 %, F0 cleanly separates Alice and Frank in all 26 turns.
2. inv_hdr is 0.1869, below vibevoice_final (0.2162) and misaki (0.2395). Most of the gap to vibevoice_final goes away on speech-only frames (0.2237 vs 0.2351), because this clip is raw with 25 % pause frames.
3. Bandwidth is narrower than the earlier VibeVoice publish clip (BW95 2250 vs 2777 Hz), with a hard ceiling around 7.7–8 kHz.
4. Speaker turns are visible as non-overlapping per-turn steps in signal (Frank 0.197–0.338 occupancy vs Alice 0.147–0.196). Quality goes the other way, and all six quality dips are in Frank turns.
5. The quality layer is saturated at the 0.12 threshold (99 % occupancy, including silence), so it carries almost no point-count information. Variant Q is offered as a way around this.
6. The braille reading works as a pause detector: 100 % of half-seconds inside inter-turn silences come out flat on signal, tonality and confidence, and every one of the 25 pauses shows a flat half-cell.

## 9. Files and scripts

- Scripts: `scripts/cube_spectrogram_3d.py` (unchanged), `scripts/library_cube_generic.py` (new, same algorithm as rev2), `scripts/bitdot_braille_analysis.py` (new), `scripts/bitdot_braille_extras.py` (new)
- Outputs: `spectrograms/bitdot_braille/`. That covers the cube PNG/HTML/PLY/JSON, `library_bitdot_braille_vibevoice_cube3d.json`, the spectrogram, channel, strip and timeline PNGs, `_strip.json`, `_analysis.json` (all metrics, per-turn and per-second data, and every braille cell) and `_extras.json` (silence test, bandwidth, slices, dips).
- Audio and script: `audio/bitdot_braille_vibevoice.wav`, `audio/bitdot_braille_vibevoice.synth.json`, `podcast_bitdot_braille.txt`
- ASR: `transcripts/nemotron_asr_bitdot_braille_vibevoice.{json,txt}`, `transcripts/bitdot_braille_wer.json`
