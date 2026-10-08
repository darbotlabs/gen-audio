# Livetile faces contract (v0, for review)

Status: **draft for Optimus review.** This file only describes behavior; it adds no code. After Optimus
signs off, the cloud agent implements the reducer and `facts()` parts in core on PR #4. Once #4 reaches
GO, it merges forward into this line with `--no-ff`, and the desktop work (faces from ruling 3, T17)
builds on top of that. One module, one owner: the reducer and `facts()` belong to core (PR #4). The
desktop only renders what they return.

Inputs:
- Optimus rulings, 2026-10-08:
  - first set: 1 (the glyph only flips), 2 (no other flip triggers; glyph aria and corner), 3 (Audio Clips faces)
  - second set: 2 (faces are an ordered list in the reducer), 3 (`facts()` is extended in core)
- `/workspace/gen-audio-tauri-brief/LIVETILE_OBJECT_MODEL_CONVERGED.md`. This contract follows
  rebuttals 1, 3 and 4, v1 FINAL amendments 1 and 3, and T17.
- PR #4 at `f750681`, which is what this contract extends. Every symbol below is cited by file and name:
  - `crates/gen-audio-core/src/viewport.rs`:
    - types and constants: `Action::Flip { view, face, section }`, `FlipState { face, section }`,
      `Viewport.flipped`, `ReduceError::invalid` (code -32602), `PARTIAL_BELOW` (0.95)
    - `Viewport::release()`, `Viewport::apply()`, `Viewport::snapshot()` (`ui.flipped`)
    - `facts(uid, kind, title, honesty, media)`, which today returns two sections, `identity` and `honesty`
    - `facts_snapshot()`, `adaptive_card()`
    - `validate_coverage(&CoverageInput) -> CoverageOk { covered_s, of_s, ratio, partial }`
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
  - `cubes` → `model_cubes`. These are today's E4 Cube-tab rows (`library-assets.ts` `modelCubes`); the
    content moves into core.
  - `relations` → `relations`
- Connector: `connector` → `identity`, `honesty`, `connector`; `relations` → `relations`.
- Voice profile: `profile` → `identity`, `honesty`, `profile`; `persona` → `persona`;
  `relations` → `relations`.

Kinds that appear only in dev docs (`SpectrogramPanel`, `Cube3D`, `PodcastCast`, `BenchmarkCompare`,
`ServeHealth`) are not typed by this contract (open question Q6).

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
| `cubes` (voice model) | `catalog::VoiceModel.unavailable` is false, or at least one Library clip names the model. If the engine is unavailable and has no clip, the face is omitted. |
| `persona` | the profile has at least one of domain, accent, traits or refs |
| `relations` | the view has at least one link (§5.6), whatever its status |

Worked examples on the release deck:
- `lib-misaki-kokoro`: M = 5.
- `lib-magpie`: M = 2 (`clip`, `relations`; the relations face has the voice model link).
- `conn-mcp`: M = 1 if it has no link. That case is open question Q2.

## 3. Reducer state and actions (core, `viewport.rs`)

### State
- Per card kind, a constant ordered face table (§1), plus `faces_for(view) -> Vec<Face>` after the
  omission rule (§2).
- Per view, `face_index: usize`. The default is 0, so a fresh view shows its first face.
  - This replaces `FlipState { face, section }` in `Viewport.flipped`.
  - A view that was never flipped has no entry and reads as index 0.
- `face_index` is per-view UI state, like `focus`, `clock` and `playing`. It is volatile, and it is
  never part of `facts()`.
- Views: PR #4's `Viewport::release()` builds views only for Library clips (`view:lib-*`) and profiles
  (`view:profile-*`). This contract also needs `view:engine-*` (home `slide:models`) and `view:conn-*`
  (home `slide:connectors`), so engine and connector tiles get typed faces (open question Q3).

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
- `section` is what PR #4 uses today to pick a facts section on the back. How it maps is open question Q1.
  Until that is ruled, `section` is accepted only together with the `back` alias and is echoed back
  without being applied, so existing callers keep working.

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

## 4. What `viewport_get` returns

`viewport_get` stays `snapshot_global()` plus `cursor`, `gap` and `resync` (`control.rs` `viewport_get`).

- `views[]` gains one **non-volatile** field: `faces: [{id, name}]`, the applicable list from §2.
- `ui` gains `faces`. It is **volatile** and keyed by view id, and every view has an entry:
  ```json
  "ui": {"faces": {"view:lib-misaki-kokoro": {"tileId": "lib-misaki-kokoro", "face_id": "clip", "face_index": 0, "face_count": 5}}}
  ```
- `ui.flipped` (PR #4) stays for one release as a derived read: `{"face": "front"}` at index 0 and
  `{"face": "back"}` otherwise, so readers of the old shape keep working (open question Q4).
- When #4 merges forward, `viewport_get` on this line must keep `cube_mode`, `cube_compare`, `cube_seq`
  and `cube_modes` from PR #7. `ui.faces` must not collide with them (open question Q5).

## 5. `facts()` sections (core, non-volatile)

`facts()` is extended in core. It reads the asset envelope through `asset_catalog`, so the desktop never
derives a fact.

**New signature (proposal):** `facts(view_asset: &FactsInput) -> Vec<Section>`, where `FactsInput`
carries `uid`, `kind`, `title`, `honesty`, `media` and the asset envelope from `asset_catalog::require`.
`identity` and `honesty` keep their current shape, so `adaptive_card()` and `card_export` keep working;
they gain only the new `status`.

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
| `inv_hdr` | number | ratio, unitless | `fields.inv_hdr_ppm / 1e6`. This is the "loudness ratio" in ruling 3 (open question Q7). |
| `coverage` | `{covered_s, of_s, ratio}` | s, s, 0..1 | `validate_coverage` with `selector_start = 0`, `selector_end = fields.covers_ms/1000`, `clip_duration_s` = clip `fields.duration_ms/1000`, `recorded_source_duration_s = fields.duration_ms/1000`, `sec_per_bin = body.bin_seconds`, `clip_in_src` = `src` contains the clip uid. The function is **imported, never re-implemented**. |
| `partial` | `{covered_s, of_s}` or absent | s | `CoverageOk.partial` |
| `sec_per_bin` | number | s | `body.bin_seconds` |
| `shape_f_t` | `[int, int]` | bins | `fields.freq_bins`, `fields.time_bins` |
| `cube_revision`, `layer_method` | int, string | — | `fields` |
| `cube_json` | `{path, sha256}` | — | `media[role=cube_json]` |

- **The partial rule.** If `coverage.ratio < PARTIAL_BELOW` (0.95), then `status = "partial"`, `partial`
  is set, and the face is **led** by the qualifier `partial (<covered_s> s of <of_s> s)`.
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
| `formula` | `{ref: {generator, generator_sha256, symbol, layer_method}, text}` | — | `ref` is `provenance.generator` and `generator_sha256` of the cube plus `symbol` `compute_layers`. `text` is `pending` until Q8 is ruled. |

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
- **`model`**: `engine_id`, `label`, `waveform` (bool; `false` means G2P only), `synth_adapter` (bool),
  `unavailable` (bool), `offline_reason` (string or absent). Sources: `catalog::VoiceModel` and the
  `voice_model` envelope.
- **`model_cubes`**: a list of `{clip_uid, clip_title, cube_uid, revision, shape_f_t, inv_hdr, layer_score, cube_json}`.
  The rows are the ones `library-assets.ts` `modelCubes` builds today; that logic moves into core.
- **`connector`**: `connector_id`, `mode` (`live | local | mock | token_present`), `authenticated` (bool),
  `detail`. The source is open question Q3.
- **`profile`**: `persona_id`, `agent_name`, `voice_model`, `tone`, `purpose`. Source:
  `catalog::voice_profile_value`.
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
  - comparison cubes (`compare_to`, `layer_method != library_r3`)
  - the bound `card` (`relations.bound_to`)
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

The snapshot changes only when the asset, its `display_rev` or its media presence changes (converged
rebuttal 4). A flip or a play never rewrites a snapshot.

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
5. Aliases:
   - `ui_flip {"tileId": "lib-misaki-kokoro", "face": "back"}` gives `cube`, 1
   - `{"flipped": false}` gives `clip`, 0
   - `ui_flip {"tileId": "lib-misaki-kokoro"}` with no selector gives `cube`, 1, the legacy default
6. `ui_flip {"tileId": "lib-misaki-kokoro", "face": "waveform"}` → JSON-RPC error **-32602**. The message
   names `clip, cube, layers, spectrogram, relations`, and a following `viewport_get` shows the state unchanged.
7. Omitted face: `ui_flip {"tileId": "lib-magpie", "face": "cube"}` → **-32602**, naming `clip, relations`.
   `viewport_get` shows `face_count: 2` for `view:lib-magpie`.
8. Engine and connector: `ui_flip {"tileId": "engine-kokoro", "next": true}` → `cubes`, 1, 3.
   `conn-claude` reads back with its own `face_count`.
9. Two selectors: `ui_flip {"tileId": "lib-misaki-kokoro", "next": true, "face": "cube"}` → -32602.
10. Snapshot stability: steps 2–5 leave every `views[].snapshot` byte-identical (§7).
11. **face_index stability under data mutation (Q10):** with `view:lib-misaki-kokoro` on face
    `cube` (index 1), run `library_harvest {clipId: "lib-misaki-kokoro", personaId: "anton", apply: true}`
    (and any other non-Flip mutation under §7.1). `viewport_get` still reads
    `face_id: "cube", face_index: 1`. A following `ui_flip {next: true}` is the only step that moves it.

## 9. Open questions

- **Q1. `section` (PR #4 `Action::Flip.section`).** The faces in §1 make "back + section" redundant.
  Should `section: "<id>"` with the `back` alias select the face that renders that section (for example
  `honesty` selects `clip`), or should it be deprecated with a logged warning, like the bare slide alias
  (ruling C5)?
- **Q2. One face (M = 1).** For example a connector with no relation link. What does the glyph do?
  - Proposal: it stays in the corner with the label `Flip card, face 1 of 1: <name>`, and activating it
    is a no-op. The `back` alias is -32602 and `next` stays at index 0.
  - Alternative: give every connector a second face.
- **Q3. Engine and connector views, and the connector facts owner.**
  - PR #4's `Viewport::release()` has no `view:engine-*` or `view:conn-*`.
  - Connector status lives in `crates/gen-audio-connectors`, not in core. Which module owns the
    `connector` section?
- **Q4. `ui.flipped`.** Keep it as a derived read for one release, as §4 says, or drop it when `ui.faces`
  lands?
- **Q5. `viewport_get` merge.** This line (PR #7) returns the cube mode at the top level, and #4 returns the
  reducer snapshot. On the forward merge, does the cube mode move into `ui` (`ui.cube_mode`) or stay top level?
- **Q6. Dev-only kinds** (`SpectrogramPanel`, `Cube3D`, `PodcastCast`, `BenchmarkCompare`, `ServeHealth`).
  Type them, or keep two faces `summary` / `details` until they leave the dev doc?
- **Q7. "Loudness ratio."** No field has that name. §5.2 maps it to `inv_hdr` (from `fields.inv_hdr_ppm`).
  Confirm, or name the field that is meant.
- **Q8. Layer formula text.** The formulas are written only in `src/gen_audio/cube_layers.py` (its
  docstring and `compute_layers`), and cube uid identity is that file's `generator_sha256`. Emitting the
  text from the generator changes the sha, which moves the five library cube uids, and no uid may move.
  Copying the text into core breaks "shared math is imported, never copied." The proposal is that
  `formula.ref` (generator, sha, symbol) is real now and `formula.text` stays `pending` until Optimus
  picks a source. Options:
  - (a) a formulas sidecar emitted by a *separate* generator, verified against `cube_layers.py` by a test;
  - (b) the next intentional cube revision, with an announced uid move;
  - (c) ref only.
- **Q9. Error data.** `ReduceError` carries only `{code, message}`. Should the valid face ids also be a
  JSON-RPC `error.data.valid_faces` array, so agents don't parse the message?
