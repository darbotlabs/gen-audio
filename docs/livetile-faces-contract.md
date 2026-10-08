# Livetile faces contract (v1.1)

Status: **v1.1b — C11-M1/M2 + L1–L5 (2026-10-08).** Behavior-only; no code on this line. Symbol
citations below still name PR #4 at `99ba67e` (do not chase later moves). Behavioral claims were
re-checked at `fb07d91` (PR #4 head / `cursor/no-stubs-pipeline-bcbb`); every citation still holds.
The cloud agent implements the reducer and `facts()` parts in core on PR #4. Once #4 reaches GO, it
merges forward into this line with `--no-ff`, and the desktop work (faces from ruling 3, T17, C-M4)
builds on top of that. One module, one owner: the reducer and `facts()` belong to core (PR #4). The
desktop only renders what they return. A spec is only true for the tip it cites.

Inputs:
- Optimus rulings, 2026-10-08:
  - first set: 1 (the glyph only flips), 2 (no other flip triggers; glyph aria and corner), 3 (Audio Clips faces)
  - second set: 2 (faces are an ordered list in the reducer), 3 (`facts()` is extended in core)
- `/workspace/gen-audio-tauri-brief/LIVETILE_OBJECT_MODEL_CONVERGED.md`. This contract follows
  rebuttals 1, 3 and 4, v1 FINAL amendments 1 and 3, and T17.
- PR #4 at `99ba67e` (read-only tip this contract cites; do not chase later moves). Every symbol below is cited by file and name:
  - `crates/gen-audio-core/src/viewport.rs`:
    - types and constants: `Action::Flip { view, face, section }`, `FlipState { face, section }`,
      `Viewport.flipped`, `ReduceError::invalid` (code -32602), `PARTIAL_BELOW` (0.95)
    - `Viewport::release()`, `Viewport::apply()`, `Viewport::snapshot()` (`ui.flipped`)
    - `facts(uid, kind, title, honesty, media)`, which today returns two sections, `identity` and `honesty`
    - `facts_snapshot()`, `adaptive_card()`
    - `validate_coverage(&CoverageInput) -> Result<CoverageOk, String>` (`CoverageOk { covered_s, of_s, ratio, partial }`)
    - test `t17_headless_actions_round_trip_through_viewport_get`
  - `crates/gen-audio-mcp/src/control.rs`: `ui_flip` (which accepts `tileId|view`, `flipped`,
    `face: front|back`, `section`), `flip_view`, `viewport_get` (returns `viewport::snapshot_global()`
    plus a cursor), `card_export`.
  - `crates/gen-audio-core/src/asset_catalog.rs`: `assets()`, `require()`, `uid_for_legacy()`, `tile_id_for()`.
- This branch (`cursor/livetile-faces-v2`), the desktop side that will consume the contract:
  - `apps/desktop/src/render.ts`: `setFace`, `stepFace`, `setCardFlip` (data-face / data-faces)
  - `apps/desktop/src/glyph.ts`: `flipGlyph`, `flipGlyphLabel`
  - `apps/desktop/src/library-assets.ts`: `derivedFrom`, `compareCubesFor`, `modelCubes`

## 1. Faces per tile kind

A face is `{id, name}`. `id` is a stable lowercase token: never `front`, `back` or `next`, and unique
within a kind. `name` is the label that appears in the glyph's aria-label. The faces below are listed in
order. `M` is the number of faces that apply to that view (§2).

| Tile kind (desktop `kind` / asset kind) | Faces in order (`id`: name) |
|---|---|
| Audio Clips (`LibraryClip` / `audio_clip`) | `clip`: Clip, `cube`: Cube, `layers`: Layers, `spectrogram`: Spectrogram, `relations`: Relations |
| Voice model (`EngineStatus` / `voice_model`) | `model`: Model, `cubes`: Cubes, `relations`: Relations |
| Connector (`ConnectorStatus`) | `connector`: Connector, `relations`: Relations |
| Voice profile (`VoiceProfile` / `voice_profile`) | `profile`: Profile, `persona`: Persona, `relations`: Relations |

Which `facts()` sections each face renders (§5):

- Audio Clips:
  - `clip` → `identity`, `honesty`, `clip`
  - `cube` → `cube`
  - `layers` → `layers`
  - `spectrogram` → `spectrogram`
  - `relations` → `relations`
- Voice model:
  - `model` → `identity`, `honesty`, `model`
  - `cubes` → `model_cubes` (real rows only). These are today's E4 Cube-tab rows
    (`library-assets.ts` `modelCubes`); the content moves into core. When the model is omitted
    from `cubes` (§2 wav_missing rule), this face is absent and `facts().cube` (pending /
    `wav_missing`) is what the renderer shows on the `model` face instead.
  - `relations` → `relations`
- Connector: `connector` → `identity`, `honesty`, `connector`; `relations` → `relations`.
- Voice profile: `profile` → `identity`, `honesty`, `profile`; `persona` → `persona`;
  `relations` → `relations`.

Kinds that appear only in **dev builds** (`SpectrogramPanel`, `Cube3D`, `PodcastCast`,
`BenchmarkCompare`, `ServeHealth`) get two untyped faces `summary` / `details` in the fixture deck only
(Decision Q6). They are compiled out of release; a desktop test greps the release bundle for their
absence.

### 1.1 Generate-created views (C-M3)

Every view kind gets a defined ordered face list, even when M = 1. View ids are
`view:<uid>:cube`, `view:<uid>:spectrogram` and `view:<uid>:video`, where `<uid>` is the
source clip's uid (the same shape `derived_id(uid, kind)` produces today at `99ba67e`
`viewport.rs` L848: `format!("{uid}:{kind}")`, then wrapped as `view:{…}`). They are **not**
`view:cube:*` / `view:spec:*` / `view:video:*`.

| View kind (Generate / derived) | View id | Faces in order | `facts()` sections |
|---|---|---|---|
| Cube stage (spatial bind) | `view:<uid>:cube` | `cube`: Cube | `identity`, `honesty`, `cube` |
| Spectrogram panel | `view:<uid>:spectrogram` | `spectrogram`: Spectrogram | `identity`, `honesty`, `spectrogram` |
| Video slide | `view:<uid>:video` | `video`: Video | `identity`, `honesty`, `video` |

`honesty` on a cube view may be `partial` when `validate_coverage` returns
`CoverageOk.partial` (coverage ratio < `PARTIAL_BELOW`). That is the cube view's
`honesty.partial` case; spectrogram and video views do not use `partial`.

These are not Library tiles. The omission rule (§2) still applies; a pending bind keeps the face with
`status: pending` rather than inventing pixels. **C11-L3:** there is no `video` asset kind in
`schemas/asset-object.schema.json` today (kinds: `voice_model`, `voice_profile`, `audio_clip`,
`spectrogram_2d`, `cube_ihdr`, `layer`, `podcast_script`, `transcript`, `card`, `mcp_tool`). The
`video` section is therefore **pending until a `video` kind exists**: every field reads `"pending"`,
`status: "pending"`, `reason: "no_video_asset_kind"`. Do not invent a path/sha from another kind.

## 2. Omission rule (M stays truthful)

A face is either **applicable** to a view or **omitted**. An omitted face is absent from the view's
`faces` list, absent from `face_count`, and cannot be reached by `Flip`.

Applicability is computed from non-volatile facts only (§7), so `M` never changes because something was
played, flipped, or because a job ticked. It changes only when the asset, its `display_rev`, or its media
presence changes. That matches converged rebuttal 4: `snapshot = facts(asset, media_presence)`.

- **Inapplicable means "this view can never have it."** Omit the face.
- **Applicable but not there yet means "pending."** Keep the face and render it with status `pending`.

| Face | Applies when |
|---|---|
| `clip`, `model`, `connector`, `profile` | always (the first face) |
| `cube`, `layers`, `spectrogram` | the clip's WAV is present (`media` is `present`), so a cube or spectrogram can be derived. A clip with `wav_missing` (`lib-magpie`, `lib-vibevoice`, `lib-pocket`) omits all three. |
| `cubes` (voice model) | At least one Library clip that **names the model** (§5.5 join) has its WAV present, so `modelCubes` can emit a real `cubes` row. **g2p_only** models (honesty claim `g2p_only`; `modelCubes` returns `{state:"g2p", …}`) omit `cubes` — G2P emits phonemes, not a waveform, so there is no cube row. If the engine is unavailable and has no naming clip, the face is omitted. **A model whose only clip(s) have no WAV is OMITTED from `cubes`** — no placeholder (AP-OPT-1). The model's `facts().cube` section still carries `status: "pending"`, `reason: "wav_missing"`, and the clip uid(s) / catalog tile ids; the renderer shows missing + why. Honest absence, consistent with the clip media-presence rule. Worked: `magpie` / `pocket_tts` (catalog tiles `lib-magpie`, `lib-pocket` only) omit `cubes`; `vibevoice` keeps the bitdot cube and does **not** get a pending cube section. |
| `persona` | the profile has at least one of domain, accent, traits or refs |
| `relations` | the view has at least one link (§5.6), whatever its status |

Worked examples on the release deck:
- `lib-misaki-kokoro`: M = 5.
- `lib-magpie`: M = 2 (`clip`, `relations`; the relations face has the voice model link via catalog `engine_id`).
- `engine-vibevoice`: M = 3 (`model`, `cubes`, `relations`) — `modelCubes("vibevoice")` returns one row for `lib-bitdot-braille-vibevoice` (`ga:audio_clip:gq2l5uxxj44uwnok6io3kduf5a`).
- `engine-misaki`: M = 2 (`model`, `relations`) — honesty `g2p_only`; `modelCubes("misaki")` → `{state:"g2p", clips:[lib-misaki-kokoro]}` via `provenance.g2p_model`; no cube rows → omit `cubes`.
- `conn-mcp`: M = 1 if it has no link. That case is Decision Q2. `conn-claude`: M = 1 (`face_count: 1`).

## 3. Reducer state and actions (core, `viewport.rs`)

### State
- Per card kind, a constant ordered face table (§1), plus `faces_for(view) -> Vec<Face>` after the
  omission rule (§2).
- Per view, `face_index: usize`. The default is 0, so a fresh view shows its first face.
  - This replaces `FlipState { face, section }` in `Viewport.flipped`.
  - A view that was never flipped has no entry and reads as index 0.
- `face_index` is per-view UI state, like `focus`, `clock` and `playing`. It is volatile, and it is
  never part of `facts()`.
- Views (C-M1): the release deck has **8** LibraryClip tiles. `Viewport::release()` must build
  exactly those 8 views, matched by tile id:
  - the **clip envelopes** in `apps/desktop/public/library/assets.json` (five `audio_clip` records at
    `99ba67e`: `lib-cube-explainer`, `lib-bitdot-braille-vibevoice`, `lib-kokoro`, `lib-kokoro-onnx`,
    `lib-misaki-kokoro`), plus
  - the **catalog tiles that have no envelope** (`lib-magpie`, `lib-vibevoice`, `lib-pocket`), taken
    from `catalog::library_clips()` / the release card list.
  Do **not** iterate only `catalog::library_clips()` (seven entries at `99ba67e` — misses
  `lib-cube-explainer`) and do **not** iterate only the envelopes (five — misses the three
  wav-missing tiles). **Core test (required):** after `Viewport::release()`, the set of
  `view:lib-*` ids equals the eight LibraryClip card ids in `viewport.release.json`.
- Also (Decision Q3): `view:engine-*` (home `slide:models`) and `view:conn-*` (home `slide:connectors`).
  Profiles stay `view:profile-*`.

### Actions
`Action::Flip` becomes `Flip { view: String, to: FlipTo }`, where `FlipTo` is one of:

| Form | Meaning |
|---|---|
| `Next` | `face_index = (face_index + 1) % M`. It wraps after the last face. This is what a glyph click or Enter/Space posts. |
| `Face(id)` | `face_index` = the position of `id` in `faces_for(view)` |
| `Face("front")` | alias for index 0 (kept so existing MCP callers don't break) |
| `Face("back")` | alias for index 1. If `M < 2` it is an error. |

MCP `ui_flip` arguments (`control.rs` `ui_flip`), on top of today's `tileId | uid | view`:
- `next: true`: the `Next` form.
- `face: "<id>" | "front" | "back"`: the `Face` form.
- `flipped: true|false`: kept as an alias, `true` meaning `face: "back"` and `false` meaning `face: "front"`.
  This is today's default path. A bare `ui_flip {tileId}` still means `flipped: true`, which is `back`.
- At most one of `next`, `face` and `flipped` may be given. Two or more is -32602.
- `section` is what PR #4 uses today to pick a facts section on the back. Decision Q1 (closed):
  `back` + `section: "<id>"` maps to the face that renders that section (for example `honesty` →
  `clip`). Log a deprecation for one release; then the pair is **-32602**.
- **C11-L2 — bare `{tileId, section}` without `back` / `face` / `flipped`:** at `fb07d91`
  `control.rs` `ui_flip` L629–644, `flipped` defaults to `true` and `face` to `"back"`, so
  `{tileId, section}` is accepted and applied as back+section today. Contract: log a **deprecation
  warning for one release** (same window as Q1), then the bare form is **-32602**. Do not fail-open
  forever; do not make it hard-fail before the deprecation window ends.

The reducer result, which is also the `flip` event echoed over SSE with its seq (converged 3b: TS applies
events, never actions):
```json
{"op": "flip", "view": "view:lib-misaki-kokoro", "tileId": "lib-misaki-kokoro",
 "face_id": "cube", "face_index": 1, "face_count": 5}
```

### Errors
Every error is `ReduceError::invalid`, so the code is **-32602**, and state is unchanged.
- Unknown view: `unknown view view:<x>` (as today).
- Unknown or omitted face id:
  `unknown face "<id>" for view:<tile>; valid faces: clip, cube, layers, spectrogram, relations (aliases: front, back)`.
  The list holds this view's applicable ids, in order. A face omitted by §2 is unknown for that view.
- `back` when `M < 2`: `face "back" needs 2 faces; view:<tile> has 1 (clip)`.
- More than one selector: `ui_flip takes one of next, face, flipped`.

### 3.1 Desktop must not keep a local face reducer (C-M4)

Retire the desktop's local face state (`stepFace` / `setFace` as the source of truth). The glyph posts
`Flip { next }` to `/control` and renders **only** the echoed `flip` event (converged 3b: TS applies
events, never actions). When MCP is down, the glyph is `aria-disabled` with a status message; there is
**no** local fallback flip. Until #4 merges forward, this line's `setFace`/`stepFace` remain the interim
renderer used by tests; C-M4 is the merge acceptance criterion.

## 4. What `viewport_get` returns

`viewport_get` stays `snapshot_global()` plus `cursor`, `gap` and `resync` (`control.rs` `viewport_get`).

- `views[]` gains one **non-volatile** field: `faces: [{id, name}]`, the applicable list from §2.
- `ui` gains `faces`. It is **volatile** and keyed by view id, and every view has an entry:
  ```json
  "ui": {"faces": {"view:lib-misaki-kokoro": {"tileId": "lib-misaki-kokoro", "face_id": "clip", "face_index": 0, "face_count": 5}}}
  ```
- `ui.flipped` (PR #4) stays for one release as a derived read: `{"face": "front"}` at index 0 and
  `{"face": "back"}` otherwise, so readers of the old shape keep working (Decision Q4).
- When #4 merges forward, `viewport_get` on this line must keep `cube_mode`, `cube_compare`, `cube_seq`
  and `cube_modes` from PR #7. `ui.faces` must not collide with them (Decision Q5).

## 5. `facts()` sections (core, non-volatile)

`facts()` is extended in core. It reads the asset envelope through `asset_catalog`, so the desktop never
derives a fact.

**New signature (proposal):** `facts(view_asset: &FactsInput) -> Vec<Section>`, where `FactsInput`
carries `uid`, `kind`, `title`, `honesty`, `media` and the asset envelope from `asset_catalog::require`.
`identity` and `honesty` keep their current shape, so `adaptive_card()` and `card_export` keep working;
they gain only the new `status`.

**`uid_for_legacy` step:** when a tool or view is addressed by a legacy tile/clip/persona id, resolve with
`asset_catalog::uid_for_legacy(kind, legacy_id)` (`99ba67e` `asset_catalog.rs` ~L305) **before**
`require`/`facts`. UI tools that accept `tileId|uid` try the uid path first, then this legacy map. A miss
is -32602, not a guessed uid.

Every section is:
```
{ "id": string, "status": "real" | "pending" | "partial", "facts": [Fact], ...section fields }
Fact = { "label": string, "field": string, "value": <typed> | "pending", "unit"?: string }
```

- **Status.**
  - `real`: every field of the section resolves from the catalog.
  - `pending`: at least one field cannot resolve yet, either because a derived job hasn't landed
    (`Viewport::ensure_derived` views start `pending`, converged v1 FINAL 5c) or because the source does not
    record it.
  - `partial`: only the cube rule (§5.2) and links that point at a partial cube (§5.6).
- **No pixels or samples.** No section carries pixels, samples or a stand-in value. Missing data is the
  literal string `"pending"` (AP-OPT-1).

### 5.1 `clip` (Audio Clips)

| field | type | unit | source |
|---|---|---|---|
| `uid` | string (`ga:audio_clip:…`) | — | `audio_clip` envelope `uid`. `pending` when the tile has no clip asset. |
| `duration_s` | number | s | `fields.duration_ms / 1000` |
| `sample_rate_hz` | integer | Hz | `fields.sample_rate_hz` |
| `engine` | string | — | `fields.engine` |
| `source` | string | — | `provenance.generator` |
| `wav_sha256` | string (hex 64) | — | `media[role=wav].sha256` |

The desktop draws the `Copy clip uid` button next to `uid`, and only on this face.

### 5.2 `cube` (Audio Clips)
The clip's own cube is the `cube_ihdr` whose `src` is exactly `[clip uid]`, with
`provenance.layer_method` = `library_r3`. That is the same rule as `library-assets.ts` `derivedFrom`;
comparison cubes never count.

| field | type | unit | source |
|---|---|---|---|
| `cube_uid` | string | — | envelope `uid` |
| `inv_hdr` | number | ratio, unitless | `fields.inv_hdr_ppm / 1e6`. Labelled **inverse-HDR ratio** (Decision Q7); never "loudness". Source: `fields.inv_hdr_ppm / 1e6`. |
| `coverage` | `{covered_s, of_s, ratio}` | s, s, 0..1 | `validate_coverage` (`Result`, first check `sec_per_bin > 0` at `99ba67e` `viewport.rs` ~L974) with `selector_start = 0`, `selector_end = fields.covers_ms/1000`, `clip_duration_s` = clip `fields.duration_ms/1000`, `recorded_source_duration_s = fields.duration_ms/1000`, `sec_per_bin = body.bin_seconds`, `clip_in_src` = `src` contains the clip uid. The function is **imported, never re-implemented**. |
| `partial` | `{covered_s, of_s}` or absent | s | `CoverageOk.partial` |
| `sec_per_bin` | number | s | `body.bin_seconds` |
| `shape_f_t` | `[int, int]` | bins | `fields.freq_bins`, `fields.time_bins` |
| `cube_revision`, `layer_method` | int, string | — | `fields` |
| `cube_json` | `{path, sha256}` | — | `media[role=cube_json]` |

The desktop draws the `Copy cube uid` button next to `cube_uid` on the spatial slide (Optimus Q12);
the cube glyph itself never copies.

- **The partial rule.** If `coverage.ratio < PARTIAL_BELOW` (0.95), then `status = "partial"`, `partial`
  is set, and the face is **led** by the qualifier `partial (<covered_s> s of <of_s> s)`. The on-tile
  status badge (when present) carries the same `partial` qualifier text.
  - Example: misaki at 139.04 / 139.375 = 99.8% is `real` with no qualifier (converged T19).
- **Rejection.** If `validate_coverage` rejects (its fixed reason order), the section is `pending`, every
  value is `pending`, and `reason` holds the first failing reason. A cube that lies is never shown as real.

### 5.3 `layers` (Audio Clips)
`layers` is an array of exactly four entries, in the order `signal`, `tonality`, `confidence`, `quality`.
That is the order of `cube_layers.py` `LAYER_NAMES`.

| field | type | unit | source |
|---|---|---|---|
| `name` | string | — | `layer` envelope `fields.name` (with `relations.layer_of` = the cube in §5.2) |
| `value` | number | unitless 0..1 | `body.mean`, the layer mean over the cube |
| `stats` | `{std, p50, p90, active_frac}` | unitless | `layer` envelope `body` |
| `formula` | `{ref: {generator, generator_sha256, symbol, layer_method}, text}` | — | `ref` is `provenance.generator` and `generator_sha256` of the cube plus `symbol` `compute_layers`. `text` is `pending` per Decision Q8. |

The section is `real` only when all four entries resolve and every `formula.text` resolves. Until then it
is `pending`, while the values and refs that did resolve are still shown.

### 5.4 `spectrogram` (Audio Clips)

| field | type | unit | source |
|---|---|---|---|
| `spectrogram_uid` | string | — | `spectrogram_2d` envelope with `src == [clip uid]` |
| `png` | `{path, sha256, bytes}` | —, —, bytes | `media[role=spectrogram_png]`. The **asset path and sha only, never pixels.** |
| `seconds_per_px` | number | s/px | `body.seconds_per_px` |
| `covers_s`, `duration_s` | number | s | `fields.covers_ms/1000`, `fields.duration_ms/1000` |
| `source_sha256` | string | — | `fields.source_sha256`. It must equal the clip's `wav_sha256`; if not, the section is `pending` with `reason`. |

The spectrogram is bound to the shared playhead on the desktop. That binding is transport-local and stays
out of the reducer (converged v1 FINAL 3a: no per-frame events over SSE).

### 5.5 `model`, `model_cubes`, `connector`, `profile`, `persona`
- **`model`**: `engine_id` (the `voice_model` envelope's `legacy_id` / `catalog::VoiceModel.id`),
  `label`, `waveform` (bool; `false` means G2P only), `synth_adapter` (bool), `unavailable` (bool),
  `offline_reason` (string or absent). Sources: `catalog::VoiceModel` and the `voice_model` envelope.
- **Which field makes a clip name a model (C11-M2):**
  1. **Primary (enveloped clips):** the clip envelope's `provenance.voice_model` (a `ga:voice_model:…`
     uid). That is what `modelCubes` uses today — `apps/desktop/src/library-assets.ts` L64–70
     (`clipsBy("voice_model")` filters `asset.provenance?.voice_model === model.uid`). G2P links use
     `provenance.g2p_model` the same way (L66–67 when `honesty.claims` includes `g2p_only`).
  2. **Fallback (no envelope):** catalog `engine_id` is used **only** for the three release-deck
     tiles with no `audio_clip` envelope — `lib-magpie`, `lib-vibevoice`, `lib-pocket` — to resolve
     the voice-model relation against `VoiceModel.id` / the voice_model envelope `legacy_id`.
  3. **Do not claim** that every catalog `engine_id` equals a `VoiceModel.id`. Counterexample:
     `lib-misaki-kokoro` has catalog `engine_id: "misaki_kokoro"`, which matches **no** VoiceModel;
     it names `kokoro_dayour` via `provenance.voice_model` and `misaki` via `provenance.g2p_model`.
- **`model_cubes`**: a list of `{clip_uid, clip_title, cube_uid, revision, shape_f_t, inv_hdr, layer_score, cube_json}`.
  The rows are the ones `library-assets.ts` `modelCubes` builds today; that logic moves into core.
  A model whose only clip(s) have `wav_missing` contributes **no** row and **no** placeholder
  entry here (omitted from `cubes`; AP-OPT-1). **g2p_only:** `modelCubes` returns `{state:"g2p", clips}`
  (no cube rows); the `cubes` face is omitted (§2).
- **`cube` (voice model, wav_missing case):** when every Library clip that names the model has
  `wav_missing`, `facts()` still emits a `cube` section:
  `{ "id": "cube", "status": "pending", "reason": "wav_missing", "clip_uids": [<uid>, …] }`.
  The desktop renders that the cube is missing and why. It must not invent a cube card or a
  stand-in row in `model_cubes`.
- **`connector`**: `connector_id`, `mode` (`live | local | mock | token_present | misconfigured`),
  `authenticated` (bool), `detail`. The source is Decision Q3. `misconfigured` is a valid mode at
  `99ba67e` (`validate.ts` `CONNECTOR_MODES`, `cards.rs`); facts and validators must accept it.
- **`profile`**: `persona_id`, `name` (from `Persona.name`, not a separate `agent_name` field), `voice_model`, `tone`, `purpose`. Source:
  `catalog::voice_profile_value` (emits `agentName` as the persona's name today).
- **`persona`**: `domain`, `accent`, `traits`, `refs` (strings, shown as given). Refs that are not catalog
  uids are shown as text, never as links.

### 5.6 `relations` (all kinds)
`links: [{rel, target_uid, target_kind, status, reason?}]`.

- **Status of each link:**
  - `real`: the target resolves in `asset_catalog`, and for derived links its `fields.source_sha256`
    equals the source WAV sha.
  - `pending`: the target is a derived view that hasn't landed (`ensure_derived`), or the sha check fails.
    `reason` then says which.
  - `partial`: the target is a cube whose `cube` section is `partial`.
- **Audio Clips links:**
  - `voice_model` and `g2p_model` (`provenance.*`)
  - `spectrogram_2d` and the clip's own `cube_ihdr` (via `src`)
  - comparison cubes (`layer_method != library_r3` on cubes whose `src` includes the clip; there is **no**
    `compare_to` field)
  - the bound `card`: reverse lookup via `relations.bound_to` on the card envelope (clip→card is not a
    forward field on the clip)
- **Voice model links:** clips it rendered and profiles that name it.
- **Profile links:** its voice model, plus attached clip refs that resolve to a clip uid.
- **Section status:** the worst of its links, ordered `partial` > `pending` > `real`.

## 6. The 'pending' rendering rule (desktop)

- Any fact whose value is `"pending"` renders the literal text **pending**. A section with
  `status: "pending"` says so in its face heading.
- There is no placeholder, sample, fixture, zero or blank. A fixture is never labelled Library or podcast.
- A face is never blank. If nothing on it resolves, it shows `pending`; if it can't apply, it is omitted (§2).
- The `partial` qualifier leads the Cube face (§5.2) and the relation links that point at that cube.
- The desktop renders `facts()` output only; it derives no fact. Face and section names come from
  `views[].faces` and the section ids.

## 7. Volatile and non-volatile split

| Non-volatile: in `facts()` and `views[].snapshot` | Volatile: `viewport_get.ui` / `jobs` only |
|---|---|
| `identity`, `honesty`, `clip`, `cube` (with `partial`), `layers`, `spectrogram`, `model`, `model_cubes`, `connector`, `profile`, `persona`, `relations`; `views[].faces` and so `face_count` | `ui.faces[view].face_index` and `face_id`, `focus`, `compare`, `clock` (source + committed Seek), `playing`, `slide`, job `phase` |

The snapshot changes when the asset, its `display_rev`, its media presence, **or a linked asset**
changes (converged rebuttal 4, extended). A flip or a play never rewrites a snapshot. Example: a clip's
snapshot refreshes when its cube_ihdr or spectrogram_2d lands or moves.

### 7.1 `face_index` mutations (Optimus Q10)

`face_index` changes **only** on an explicit Flip (`Action::Flip` / MCP `ui_flip` / the glyph).
Every other data mutation leaves it unchanged, including:

- `library_harvest` with `apply: true` (Attach to profile)
- rename, harvest metadata, voice-profile field writes
- cube mode / compare enter-exit, playback, seek, navigate, select
- catalog or media-presence refreshes that rewrite `views[].snapshot`

The desktop already honours this for Attach (`applyProfileUpdate` / bus `flipcard` update the
profile and never call `setCardFlip`). Core on PR #4 must keep the same invariant in the reducer.

## 8. T17 acceptance script (headless, no webview)

These run as a cargo test in core, which extends `t17_headless_actions_round_trip_through_viewport_get`,
and over MCP against `gen-audio-mcp --http 127.0.0.1:<port>` with `scripts/mcp-call.ps1 -OutFile`.
Each step reads back through `viewport_get`.

1. `viewport_get` → `ui.faces["view:lib-misaki-kokoro"]` = `{face_id: "clip", face_index: 0, face_count: 5}`;
   `views[lib-misaki-kokoro].faces` ids are `clip, cube, layers, spectrogram, relations`.
2. `ui_flip {"tileId": "lib-misaki-kokoro", "next": true}` → the result and `viewport_get` both give `cube`, 1, 5.
3. `ui_flip {"tileId": "lib-misaki-kokoro", "face": "spectrogram"}` → `spectrogram`, 3, 5.
4. `ui_flip {"tileId": "lib-misaki-kokoro", "next": true}` twice → `relations`, 4, then wraps to `clip`, 0.
5. Aliases (always include `tileId`):
   - `ui_flip {"tileId": "lib-misaki-kokoro", "face": "back"}` gives `cube`, 1
   - `ui_flip {"tileId": "lib-misaki-kokoro", "flipped": false}` gives `clip`, 0
   - `ui_flip {"tileId": "lib-misaki-kokoro"}` with no selector gives `cube`, 1, the legacy default
6. `ui_flip {"tileId": "lib-misaki-kokoro", "face": "waveform"}` → JSON-RPC error **-32602**. The message
   names `clip, cube, layers, spectrogram, relations`, and a following `viewport_get` shows the state unchanged.
7. Omitted face / face_count from data (C-M2): `lib-magpie`, `lib-vibevoice`, and `lib-pocket` have
   **no** `audio_clip` envelope in assets.json (catalog tiles only). For these three only, catalog
   `engine_id` falls back to `VoiceModel.id` / the voice_model envelope `legacy_id` (§5.5), and all
   three voice_model envelopes exist, so the voice-model relation **resolves** and **M = 2**
   (`clip`, `relations`) — the same figure §2 already gives for `lib-magpie`. Expected:
   `face_count: 2` for those three; `ui_flip {"tileId": "lib-magpie", "face": "cube"}` → **-32602**,
   naming `clip, relations` (aliases: front, back); `ui_flip {"tileId": "lib-magpie", "next": true}`
   → `relations`, 1, 2.
8. Engine and connector (C11-L1 / C11-M1):
   - `ui_flip {"tileId": "engine-kokoro", "next": true}` → `cubes`, 1, 3.
   - `engine-vibevoice`: M = 3. `ui_flip {"tileId": "engine-vibevoice", "next": true}` → `cubes`, 1, 3
     (exactly the bitdot cube; no pending cube section on the model).
   - `engine-misaki`: M = 2 (`model`, `relations`). `ui_flip {"tileId": "engine-misaki", "face": "cubes"}`
     → **-32602** (g2p_only; valid faces: `model, relations`).
   - `conn-claude` appears **once** on the release deck (`conn-claude` count = 1) and
     `ui.faces["view:conn-claude"].face_count` = **1**.
8b. Bare section deprecation (C11-L2): `ui_flip {"tileId": "lib-misaki-kokoro", "section": "honesty"}`
    (no `back` / `face` / `flipped`) logs a deprecation for one release and behaves as back+section;
    after that window it is **-32602**. Must-pass during the window; must-fail (as -32602) after.
9. Two selectors: `ui_flip {"tileId": "lib-misaki-kokoro", "next": true, "face": "cube"}` → -32602.
10. Snapshot stability: steps 2–5 leave every `views[].snapshot` byte-identical (§7).
11. **face_index stability under data mutation (Q10):** with `view:lib-misaki-kokoro` on face
    `cube` (index 1), run `library_harvest {clipId: "lib-misaki-kokoro", personaId: "anton", apply: true}`
    (and any other non-Flip mutation under §7.1). `viewport_get` still reads
    `face_id: "cube", face_index: 1`. A following `ui_flip {next: true}` is the only step that moves it.

## 8.1 Cubes omit / wav_missing (contract tests)

Reason in one line: an honest absence, consistent with the clip media-presence rule.

| Row | Kind | Assertion |
|---|---|---|
| must-pass | model whose only clip(s) have `wav_missing`: **magpie** / **pocket_tts** (catalog tiles `lib-magpie`, `lib-pocket` — no `audio_clip` envelope / no WAV) | the `cubes` / `model_cubes` list **omits** the model (no row); `facts().cube` is `{status: "pending", reason: "wav_missing", clip_uids: ["lib-magpie"]}` (resp. `lib-pocket`); the renderer surfaces missing + why |
| must-pass | **vibevoice** | keeps **exactly** the bitdot cube (`lib-bitdot-braille-vibevoice` / `ga:audio_clip:gq2l5uxxj44uwnok6io3kduf5a` → cube `ga:cube_ihdr:dqufjgk2q4nj575exlfy7ecxqe`); `model_cubes` has that one row; **no** pending `facts().cube` section for wav_missing on the model |
| must-fail | magpie/pocket with a placeholder / stub cube entry forced into `model_cubes` or the cubes list | rejected (AP-OPT-1: no stub card) |
| must-fail | vibevoice row that deletes the bitdot cube or adds a pending wav_missing section while bitdot's WAV is present | rejected |

A mutant that re-adds a placeholder cube row must turn the must-fail row red and the must-pass omit row red.
A mutant that treats vibevoice as only-wav_missing (as the old §8.1 example did) must turn the vibevoice must-pass row red.

## 9. Bus op rename (Optimus Q11)

Today the desktop bus uses `op: "flipcard"` for a voice-profile update after `library_harvest`
apply (see `apps/desktop/src/render.ts` `applyCardOp` / `applyProfileUpdate`). That name is
misleading: the op never flips.

**Contract item (cloud agent on PR #4):** rename the bus op to **`card_status`**. Keep
`flipcard` as a read/write alias for **one release**, then drop it. Do **not** change
`control.rs` on this line; PR #4 owns the rename. Desktop acceptance of the `card_status`
alias is **pending** on this line (today's desktop still keys on `flipcard` /
`applyCardOp`); the alias lands with the `--no-ff` merge of #4, not before.

## 10. Decisions (Q1–Q9) — closed

- **Q1. `section`:** `back` + `section: "<id>"` maps to the face that renders that section (for example
  `honesty` → `clip`). Log a deprecation for one release; then the pair is **-32602**. The same
  one-release deprecation applies to bare `{tileId, section}` without `back` (C11-L2); #4 at
  `fb07d91` accepts it today by defaulting `ui_flip` to back (`control.rs` L629–633).
- **Q2. M = 1:** the glyph stays in the same corner, `aria-disabled`, labelled
  `Card has 1 face: <name>`. Activating it is a no-op. `back` is -32602; `next` stays at index 0.
- **Q3. Engine/connector views:** core adds `view:engine-*` and `view:conn-*`. The connectors crate owns
  connector status; MCP passes the probed status into `facts()`. Until probed the `connector` section
  reads `pending`. Core takes **no** dependency on the connectors crate.
- **Q4. `ui.flipped`:** stays as a derived read for one release, then is removed.
- **Q5. Cube mode:** moves under `ui` (`ui.cube_mode`, `ui.cube_compare`, …). Top-level keys remain as
  aliases for one release. Owned by the reducer.
- **Q6. Dev-only kinds:** two untyped faces (`summary` / `details`) in **dev builds only**, compiled out
  of release, with a test proving absence from the release bundle.
- **Q7. Ratio label:** source is `fields.inv_hdr_ppm / 1e6`, labelled **inverse-HDR ratio**, never
  "loudness".
- **Q8. Layer formula text:** (c) now — `formula.ref` is real; `formula.text` stays `pending`. (a) later
  (formulas sidecar). Never (b) (no announced uid move, no copied math).
- **Q9. Error data:** yes — JSON-RPC `error.data.valid_faces` (applicable ids in order) plus the
  `front`/`back` aliases listed alongside.

## 11. A-M1 — forward-merge note for PR #4

At `99ba67e`, `apps/desktop/src/main.ts` still flips on attach:

- `applyFlipcard` L706 calls `setCardFlip(board, profile-…, true)` at L714 after writing refs.
- `applyControl` L756–760: `op === "flip"` (L756) calls `setCardFlip` then `applyFlipcard`;
  `op === "flipcard"` (L759) calls `applyFlipcard` (which flips).
- The Attach button path at L486 calls `applyFlipcard(body.profile)` after `library_harvest`.

**Merge note — `reportAsync` (verified cause):** at `c2782db`, `reportAsync` was a local
function in #4's `main.ts` (L116). At `99ba67e` / `fb07d91` it lives in
`apps/desktop/src/play-control.ts` (L239) and `main.ts` imports it. This line's `main.ts` was
rewritten by `4818120` (env.ts seam) and later commits and defines **no** `reportAsync` of its
own. Resolve by keeping play-control.ts as the single owner of `reportAsync`, taking #4's import,
and keeping this line's env.ts seam and never-flip Attach/flipcard behaviour.

**C11-L4 — full conflict list** (read-only `git merge-tree --write-tree` of livetile tip
`4012590adfb0ed4aeb8198817182e1044da48cf1` against PR #4 `fb07d9189840eff57d4e2e6e9a747a3a097d4dbf`;
merge-base `8dd9b4f6c87f2e938fc2b8bb95aba3ed8f50734f`). Ten content conflicts — not only `main.ts`:

1. `apps/desktop/public/library/manifest.json`
2. `apps/desktop/src/cubeview.ts`
3. `apps/desktop/src/glyph.ts`
4. `apps/desktop/src/main.ts`
5. `apps/desktop/src/render.ts`
6. `crates/gen-audio-core/src/asset_migrate.rs`
7. `crates/gen-audio-core/src/cards.rs`
8. `crates/gen-audio-mcp/src/control.rs`
9. `crates/gen-audio-mcp/src/lib.rs`
10. `docs/ASSET_OBJECT_MODEL.md`

**Resolution on the `--no-ff` merge into this line:** keep **a8cbc08's side** —
`applyProfileUpdate` / `applyCardOp` update the profile and never flip; only an explicit `flip` op flips
`args.tileId`. The main.ts happy-dom boot test (`apps/desktop/tests/main-wiring.test.ts`) is the guard:
body clicks, timers, and attach/flipcard paths must not change `face` / `face_index`.

## 12. Connector mode low

`misconfigured` is a valid connector mode (`99ba67e` `validate.ts` `CONNECTOR_MODES` and
`cards.rs`). Facts and validators must accept it.

## 13. Follow-ups (non-blocking Lows — not in v1.1)

Left for later commits; each needs its own old-behaviour red and a mutant:

- **`library_clips()` misses `lib-cube-explainer`:** `crates/gen-audio-core/src/catalog.rs` L224 `const LIBRARY` / L320 `library_clips()` returns 7 ids on livetile tip, on #4 at `99ba67e`, and on canonical `6cdaa33` — **identical blob** `7ecf8957…`. Not livetile-exclusive; do not patch here. Route with the #4 merge-forward that implements C-M1 (envelopes + catalog tiles by tile id).
- **N4:** Enter on a card body flips; the boot test sends no keys.
- **E6:** the scrub hook can seek the wrong clip.
- **L1d:** add a test that rejects a bad card uid (do not "broaden the regex").
- **La4:** one flip clears every badge on the board.
- **ui_flip on the profile tile** clears its own new badge in the same op, so nothing is announced.
