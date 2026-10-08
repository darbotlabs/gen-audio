# Asset object model (v1 draft)

One envelope for every composable Gen-Audio asset: voice models, voice profiles
(agent personas), audio clips, 2D spectrograms, inverse-HDR cubes, cube layers,
podcast scripts, transcripts, cards/livetiles and MCP tools.

| Piece | Where |
|---|---|
| Contract (source of truth, hand-written JSON Schema 2020-12) | `schemas/asset-object.schema.json` |
| Golden vectors shared by Rust and TS | `schemas/asset-object/vectors/v1.json` |
| Pinned uid of every migrated fixture | `schemas/asset-object/vectors/fixtures_v1.json` |
| Library catalog (release v1 envelopes, generated; no dev fixtures) | `apps/desktop/public/library/assets.json` |
| Dev/test-only envelopes (never shipped) | `schemas/asset-object/fixtures/assets.dev.json` |
| Shipped deck / dev fixture deck | `schemas/examples/viewport.release.json` / `viewport.example.json` |
| Rust: canonical JSON, uid, glyph, validator, media checks | `crates/gen-audio-core/src/asset.rs` |
| Rust: pure v0 -> v1 migration | `crates/gen-audio-core/src/asset_migrate.rs` |
| TS mirror (display + parity tests; the UI never mints) | `apps/desktop/src/asset.ts` |
| Regenerate catalog / vectors | `cargo run -p gen-audio-core --example build_assets` / `--example asset_vectors` |

Status: draft for Optimus/Darbot hardening. It folds in Optimus's hardening bar
and Darbot's implementation review (`ASSET_OBJECT_SCHEMA_REVIEW.md`), using the
reconciled choices listed at the end.

## 1. Envelope

```jsonc
{
  "schema_version": "1.0.0",          // semver; only the major enters the uid; unknown major rejected
  "uid_scheme": "ga1",
  "kind": "cube_ihdr",                // closed enum, snake_case (see §4)
  "uid": "ga:cube_ihdr:26plzjawolfu5es5gab5agoxte",
  "legacy_id": "lib-misaki-kokoro.cube", // old card id / tileId / clipId / persona id; never hashed
  "status": "ok",                     // ok | missing | unavailable; never hashed
  "fields":  { ... },                 // HASHED identity fields: fixed per-kind allowlist, integers only
  "media":   [{ "role", "path", "sha256", "bytes", "mime" }], // role+sha256 HASHED; path relative to library root
  "src":     ["ga:audio_clip:..."],   // HASHED derived_from parents (Merkle DAG)
  "relations": { "layer_of", "bound_to", "composes", "supersedes" }, // unhashed links
  "honesty": { "synthesized_speech", "fixture", "not_podcast", "claims": [closed vocabulary], "note" },
  "provenance": { "generator", "generator_sha256", "generator_commit", "layer_method", "engine", "voice_model", "g2p_model", "params", "created_at" },
  "display": { "title", "summary", "semantic_name", "face_name", "glyph", "display_rev" }, // display_rev: integer, unhashed
  "body": { ... },                    // per-kind payload, unhashed; float views live here
  "extensions": { "x-vendor-thing": ... } // namespaced, never hashed
}
```

The root has `additionalProperties: false`, and the Rust validator and the TS
`checkEnvelopeShape` reject the same things on their own (D1): an unknown root
key (`unknown_root_key`), a missing `display`/`display.glyph` (`missing_glyph`),
an empty or over-long title (`bad_title`), an unknown display key
(`unknown_display_key`), and a `legacy_id` outside `[A-Za-z0-9_.-]{1,96}` or
containing `..` (`bad_legacy_id`, so no `/` and no traversal).

**`display.display_rev` (C2)** is an optional non-negative integer inside
`display` (absent = 0). It counts renames/retitles for UI caches and is never
hashed, so bumping it never changes the uid (`display_rev_bump_keeps_uid`); a
non-integer is `bad_display_rev`. That includes the token `3.0`: Rust and TS
(`checkEnvelopeShape` with `floatTokenPaths` from the raw JSON text) reject it,
but JSON Schema cannot tell `3.0` from `3`, so Ajv and the jsonschema crate
accept it (vector `display_rev_integral_float`, `schema_valid: true`). Per-kind dispatch is
`allOf: [{ if: {properties:{kind:{const:K}}, required:["kind"]}, then: {...$defs/K_fields, K_body, uid pattern, media roles} }]`.
This is equivalent to a `oneOf` over `$defs/<kind>` (the `kind` enum is closed),
but ajv and the jsonschema crate report much more readable errors. Payloads reuse
the existing schemas instead of forking them: a `voice_profile` body is
`voice_profile.schema.json`, and a `card` body is `card-viewport.schema.json#/$defs/card`.

## 2. Identity, preimage and uid

The uid hashes an allowlisted identity projection only:

```text
identity = {"kind": K, "schema_major": 1, "fields": {...},
            "media": [{"role": r, "sha256": h} ...]   sorted by (role, sha256),
            "src":   [parent uids ...]                sorted; duplicates rejected (duplicate_src), max 16}
preimage = UTF8("ga-asset-v1") || 0x00 || JCS(identity)
digest   = SHA-256(preimage)
uid      = "ga:" K ":" base32(digest[0..16])        RFC 4648 alphabet a-z2-7, lowercase, no padding
```

- **JCS (RFC 8785).** The identity projection is restricted so that JCS stays
  trivial and identical in Rust and TS.
  - Integers only, within ±(2^53−1). Use `duration_ms`, `sample_rate_hz`,
    `bin_frames` and `inv_hdr_ppm` (0.070211 becomes 70211). Floats such as
    0.1, 1e-7 and 139.375 are rejected (`float_in_identity`), as are -0
    (`negative_zero`), NaN/Inf and `null` (omit the key instead).
  - **Integral floats are rejected too (D2).** `139375.0` and `1e3` are
    `float_in_identity`: identity integers are JSON integer *tokens*, and the
    minter only writes integer tokens. Rust sees such tokens as f64 and rejects
    them in `canonicalize`/`check_fields`; JS cannot tell `1.0` from `1` after
    `JSON.parse`, so TS reads identity text through `parseIdentityJson`, which
    rejects fraction/exponent tokens before parsing. JSON Schema cannot express
    this (2020-12 `integer` admits `1.0`), so the schema stays silent and the
    validators own it (`integral_float_139375_0`, `integral_float_exponent_1e3`).
  - Keys are printable ASCII (`non_ascii_key`), and field names are snake_case.
    UTF-8 byte order therefore equals the UTF-16 order JCS specifies.
  - Strings must be NFC (`non_nfc_string`) and contain no C0 controls or DEL
    (`control_character`). Lone surrogates are rejected; Rust cannot even parse
    them. Writers NFC-normalize at ingest (`normalize_nfc`); hashing never
    normalizes, it rejects. With no controls, the only escapes are `\"` and `\\`.
- **Never in identity:** uid, glyph, titles/labels, summaries, status, notes,
  timestamps, any path or URL (`absWav`, `wav`, cube `source_wav`, `*Url`),
  honesty text, UI state, extensions, and forward or associative links
  (`relations`). A rename or retitle keeps the uid
  (`title_change_keeps_uid`, `v0_rename_keeps_uid`).
- **Media:** the sha256 of the raw file bytes (not decoded PCM). Changing the
  bytes changes the uid (`media_b_changes_uid`). Writers never rewrite library
  WAVs in place.
- **`src` is derived_from:** a cube or spectrogram points at its audio_clip,
  and a layer points at its cube. Because a uid cannot contain itself, the
  graph is acyclic by construction. Associative edges (profile→cube, card→asset)
  live in unhashed `relations` and are DFS-checked for cycles (depth cap 16,
  fan-out 8).
- **uid text:** 128 bits as 26 chars. The last char carries 3 data bits plus
  2 pad bits, which must be 0, so it is one of `a e i m q u y 4`. The grammar is
  `^ga:(voice_model|voice_profile|audio_clip|spectrogram_2d|cube_ihdr|layer|podcast_script|transcript|card|mcp_tool):[a-z2-7]{25}[aeimquy4]$`.
  The longest uid is 44 chars, which fits the existing 80-char ref charset
  (`control.rs`). `uid_scheme: "ga1"`. Rehashing in place is never allowed: a
  new scheme or major mints a new uid plus `relations.supersedes: [old uid]`.
- **Worked example (real misaki cube):**
  - `JCS(identity)` = `{"fields":{"bin_frames":8448,"covers_ms":139040,"cube_revision":3,"duration_ms":139375,"freq_bins":102,"generator_sha256":"68d0f9ff…71af9",...,"layer_method":"library_r3",...},"kind":"cube_ihdr","media":[{"role":"cube_json",...},{"role":"cube_png",...}],"schema_major":1,"src":["ga:audio_clip:vtwxksrsuci7zygslimzfy7kdy"]}`
  - digest = `d79ebca4…9090`
  - uid = `ga:cube_ihdr:26plzjawolfu5es5gab5agoxte`
  - glyph = `⣗⢞`

  Darbot's earlier worked example (`…ay76z`, built from the old kokoro_onnx
  cube under a 130-bit encoding) no longer applies: this spec takes 128 bits and
  zero pad bits.

### Identity integers: rounding (normative, B1)

Every identity integer derived from a measurement uses **round half up**.
All inputs are non-negative, so this is the same as half away from zero.
Negative or non-finite inputs are an error (`bad_rounding_input`).

| field | from | rule | implementation |
|---|---|---|---|
| `audio_clip.duration_ms`, `spectrogram_2d.duration_ms`, any `*_ms` from frames | frames, sample rate | `(frames·1000 + rate div 2) div rate`, exact integers | Rust `ms_from_frames`, TS `msFromFrames`, Python `gen_audio.identity.ms_from_frames` |
| `spectrogram_2d.covers_ms` | columns·hop frames | same integer rule | same |
| `cube_ihdr.duration_ms` | `duration_s` (float view) | round half up of the IEEE-754 double `duration_s × 1000` | Rust `round_half_up`, TS `roundHalfUp`, Python `gen_audio.identity.round_half_up` |
| `cube_ihdr.covers_ms` | `cube_covers_s` | round half up of `cube_covers_s × 1000`; without it, the integer rule on `time_bins·bin_frames` | same |
| `cube_ihdr.inv_hdr_ppm` | `inv_hdr` | round half up of `inv_hdr × 1e6` | same |
| `cube_ihdr.bin_frames` | cube JSON `downsample_sf_st[1]` and the STFT hop | `downsample_sf_st[1] × hop`, exact integers (`asset_migrate.rs`, first branch) | Rust `asset_migrate` |
| `cube_ihdr.bin_frames` (inferred, second branch) | cube JSON `duration_s` (float), `sr`, `time_bins` | `round_half_up(fl(fl(duration_s × sr) / time_bins))`: two binary64 ops, multiply first, then divide; never from frames | Rust `bin_frames_inferred`, TS `binFramesInferred`, Python `gen_audio.identity.bin_frames_inferred` |

**`bin_frames` (B1′).** `asset_migrate.rs` takes the branches in this order:
when the cube JSON has `downsample_sf_st` and a hop is known, `bin_frames =
downsample_sf_st[1] × hop`; otherwise it is inferred from the cube JSON's own
float `duration_s` as above. Since E4 every shipped Library cube is rev 3
(`gen_audio.cube_layers`, `layer_method: library_r3`) and takes the first
branch; the inferred branch remains for cube JSON without `downsample_sf_st`
(the older library cubes took it before E4). The
two-step order is normative: `157.134 s × 24000 = 3771215.9999999995`,
`/ 96 = 39283.49999999999` → **39283** (vector `bin_frames_inferred`, asserted by
cargo, npm and pytest). Exact rational arithmetic (or frames: 3,771,216 / 96 =
39283.5) gives 39284. (Before E4 this pinned `lib-cube-explainer`'s cube uid,
`ga:cube_ihdr:bcuw4m76pyqanslfiugnvlxnda`; that cube is now rev 3.)

"The double product" in the other rows is one IEEE-754 binary64 multiply, so
Rust, TS and Python get bit-identical inputs to the rounding step. Examples (all in
`v1.json` → `rounding`, asserted by cargo test, npm test and pytest):
24008 frames @ 16 kHz = 1500.5 ms exactly → **1501** (the tie); 33447 @ 24 kHz
= 1393.625 → 1394; 12 @ 24 kHz = 0.5 → 1; 11 @ 24 kHz → 0. The mint vectors
`clip_tie_half_ms_16k` and `clip_non_whole_ms_24k` carry `derived_from`
frames so both languages re-derive `duration_ms` before hashing. None of the
63 library uids changed under this rule.

### Per-kind identity fields (`fields`, allowlist)

| kind | required | optional | media roles | src |
|---|---|---|---|---|
| voice_model | model_id, waveform | | | |
| voice_profile | persona_id, voice_model (uid), tone, purpose, domain, accent, traits, refs | | profile_json | |
| audio_clip | engine, sample_rate_hz, duration_ms | channels (1..32) | wav | |
| spectrogram_2d | source_sha256, sample_rate_hz, n_fft, hop_frames, n_bands, width_px, height_px, duration_ms, covers_ms, db_floor, colormap | | spectrogram_png | exactly 1 audio_clip |
| cube_ihdr | source_sha256, sample_rate_hz, bin_frames, time_bins, freq_bins, duration_ms, covers_ms, inv_hdr_ppm, cube_revision, n_points | n_fft, hop_frames, generator_sha256, layer_method | cube_json, cube_png | exactly 1 audio_clip |
| layer | name (signal/tonality/confidence/quality), index | | | exactly 1 cube_ihdr (= relations.layer_of) |
| podcast_script | format, n_turns, n_words | | script_txt | |
| transcript | language, n_words | | transcript_json, transcript_txt | (audio_clip) |
| card | card_id, view (old card kind) | | | |
| mcp_tool | name, input_schema | | | |

### Cube identity: generator content, not history

A Library cube's uid says which generator **bytes** made it, never which
commit:

- `gen_audio.cube_layers` writes `provenance.generator_sha256` into the cube
  JSON: the sha256 of `src/gen_audio/cube_layers.py` with CRLF normalized to
  LF, so a Windows `core.autocrlf=true` checkout hashes the same. The JSON also
  keeps `generator` (the path), `layer_method` (`library_r3`) and `params`.
- The cube JSON is hashed as the `cube_json` media, and the migration also
  copies `generator_sha256` and `layer_method` into `fields`. Both routes put
  the generator's content and method in the preimage; `params` reach it
  through the cube JSON bytes.
- No commit SHA is written into the cube JSON or `fields`. A rebase, squash or
  cherry-pick keeps every cube uid; editing the generator changes them after a
  regen. `tests/test_cube_layers.py` and `build_assets` fail with "regenerate
  cubes" when the generator no longer hashes to the recorded value.
- The commit is information only. `cube_revision.py manifest
  --record-generator-commit`, run after the regen is committed, writes the
  newest commit whose generator file has those bytes to the manifest cube block
  (`generator_commit`). `build_assets` copies it into the cube envelope's
  `provenance`, which is unhashed. A test checks that a recorded commit holds
  those bytes, and skips when the commit is not in the clone.

**Media roles (B2).** Each role appears **at most once** per envelope
(`duplicate_media_role`; schema `contains` + `maxContains: 1` per role; also
rejected at mint time, so a duplicate can never reach a uid). Required roles:

| kind | required | optional | missing → |
|---|---|---|---|
| audio_clip | wav | | `clip_missing_wav` |
| spectrogram_2d | spectrogram_png | | `missing_media_role` |
| cube_ihdr, real (`claims` has `library_cube`, `fixture: false`) | cube_json **and** cube_png | | `missing_media_role` |
| cube_ihdr, pending (no `library_cube` claim yet) | cube_json | cube_png | `missing_media_role` |
| podcast_script | script_txt | | `missing_media_role` |
| transcript | one of transcript_json / transcript_txt | the other | `missing_media_role` |

"Real" and "pending" are not new honesty values: a cube is **real** when its
honesty says it is analysis of a real clip (`claims` contains `library_cube`
and `fixture` is false, see C3) and **pending** otherwise. The schema encodes
this as a nested `if/then` inside the cube_ihdr branch; Rust
(`check_required_media`) and TS (`checkEnvelopeShape`) raise
`missing_media_role` (vectors `cube_real_with_json_and_png`,
`cube_real_missing_png`, `cube_pending_missing_png`). Every library cube
(misaki, cube-explainer, kokoro-onnx, bitdot) carries both roles.
| voice_profile | | profile_json | |
| voice_model, layer, card, mcp_tool | (no media) | | |

**Pipeline lineage (for P4):** each derived step mints its own asset with
`src` set to its parent uids. For example: improve → audio_clip (src: original
clip), spectrogram_2d (src: clip), cube_ihdr (src: clip), layer (src: cube).
Any re-encode is a new asset with a new uid, and lineage stays queryable
through `src`.

## 3. Glyph

- `glyph = U+2800+digest[0] U+2800+digest[1]` (2 braille cells, 16 bits).
  Bit *k* lights dot *k+1*:
  - bit0 = dot1 (row 1, left), bit1 = dot2, bit2 = dot3
  - bit3 = dot4 (row 1, right), bit4 = dot5, bit5 = dot6
  - bit6 = dot7 (row 4, left), bit7 = dot8 (row 4, right)
- The glyph can be recovered from the uid text (its first 16 bits), so the UI
  never hashes anything.
- It is a visual hint, never identity: only 65,536 values exist, so
  collisions are expected. Resolve only by uid.
- **Rendering.**
  - The UI draws an SVG from the bits: lit dots are filled, empty dots are
    faint outlines, so `0x00` (`glyph_zero_byte`) is still visible. The
    Unicode string is the text form.
  - The hue is a fixed CSS class per kind (`ga-kind-<kind>`, plus
    `ga-kind-unknown`), never derived from the hash.
  - The SVG is `aria-hidden`; the badge's `aria-label` and tooltip carry the
    kind and the uid (copy on click).
  - No style attributes are used (CSP `style-src 'self'`).

## 4. Kinds and back-compat

- **Kinds:** `voice_model, voice_profile, audio_clip, spectrogram_2d, cube_ihdr,
  layer, podcast_script, transcript, card, mcp_tool`. Tokens are snake_case
  with no `/` or capitals, so uids pass `control.rs`'s ref charset.
  `voice_profile` covers the agent persona. Adding a kind is a schema minor
  bump; retired tokens are never reused.
- **Migration:** `migrate_to_v1` is pure. A document with no `schema_version`
  is treated as v0, `1.x` passes through, and any other major is
  `unknown_major`. `migrate_v0_to_v1` maps:
  - manifest clip →
    - `audio_clip`:
      - `fields{engine, sample_rate_hz, duration_ms, channels}` come from the
        WAV header.
      - `body{duration_s, wav_url, n_words, n_turns}`.
      - `absWav`, `wav` and `synth_wall_s` are dropped.
    - its nested `cube` → a separate `cube_ihdr` with `src:[clip]`, plus 4
      `layer` assets.
  - Unavailable "clips" (Magpie, VibeVoice, Pocket) are **not** audio_clips:
    they fold into `voice_model.body.availability`, so "no fake audio" is
    structural.
  - VoiceProfile → `voice_profile`:
    - `fields` hold the persona text plus the voice_model uid.
    - `body` is the VoiceProfile document itself.
    - `cubeJsonUrl` becomes `relations.bound_to: [cube uid]`.
  - Viewport card → `card` with `fields{card_id, view}`, where the old card
    `kind` becomes `fields.view`. `viewport.example.json` keeps `id` and gains
    `uid` alongside it, and it still validates against
    `card-viewport.schema.json`.
  - The misaki clip's voice model is `kokoro_dayour` (dayour/kokoro
    `KPipeline.generate_from_tokens`), with `provenance.g2p_model` = misaki.
  - Cubes older than rev 2 store no `bin_frames`, so it is inferred as
    `floor(duration·sr / time_bins)` and flagged in
    `provenance.params.bins_inferred_from_shape`.
- **Legacy index:** `assets.json.legacy_index` maps `"<kind>:<legacy_id>"` →
  uid; `fixtures_v1.json` pins it in CI.

## 5. Honesty invariants

Some invariants are expressed in the schema (S); the Rust validator enforces
the rest (V). Every invariant has a failing vector in `v1.json`:

| invariant | where | error code |
|---|---|---|
| `fixture:true` ⇒ `synthesized_speech:false` | S+V | `fixture_claims_speech` |
| voice_profile: `synthesized_speech:false`, `not_podcast:true` | S+V | `profile_claims_speech`, `profile_not_marked_not_podcast` |
| synthesized audio_clip needs a wav sha256, `provenance.engine` and a `provenance.voice_model` uid; status ok or missing | S+V | `clip_missing_wav`, `clip_missing_engine`, `clip_missing_voice_model` |
| cube_ihdr / spectrogram_2d: exactly one `src` audio_clip | S+V | `derived_needs_one_clip` |
| …its `source_sha256` equals the clip's wav sha256 | V (set) | `source_sha_mismatch` |
| …\|duration_ms − clip.duration_ms\| ≤ one bin (`ceil(bin_frames·1000/sr)`) | V (set) | `duration_drift` |
| …`covers_ms ≤ duration_ms` | V | `covers_exceeds_duration` |
| layer: `layer_of` is exactly one cube_ihdr and equals `src`; name is signal/tonality/confidence/quality | S+V | `layer_needs_cube`, `layer_src_mismatch`, `bad_layer_name`, `layer_cube_missing` |
| voice_profile voice ref is a voice_model uid (not a connector id or pack name) | S+V | `bad_voice_ref` |
| mcp_tool carries no secrets or env values (reuses `redact::redact_secrets`) | V | `mcp_tool_secret`, `mcp_tool_env` |
| `claims` is a closed vocabulary | S+V | `unknown_claim` |
| uid recomputes; glyph matches; uid prefix = kind | V (S for prefix) | `uid_mismatch`, `glyph_mismatch`, `kind_mismatch` |
| root keys closed; display has a 1..160 char title and the glyph; `display_rev` integer; `legacy_id` safe | S+V+TS | `unknown_root_key`, `missing_glyph`, `bad_title`, `unknown_display_key`, `bad_display_rev`, `bad_legacy_id` |
| media role at most once; required roles per kind | S+V+TS | `duplicate_media_role`, `missing_media_role`, `clip_missing_wav` |
| `channels` 1..32 | S+V+TS | `bad_field_type` |
| `src` ≤ 16 without duplicates; each relation list ≤ 8 (fan-out) | S+V+TS | `duplicate_src`, `fan_out_exceeded` |
| `body.wav_url` = `/library/` + a valid media path (same segment rules) | S+V+TS | `bad_wav_url` |
| every relation target (`composes`, `bound_to`, `supersedes`, `layer_of`) resolves in the set | V (set) | `dangling_relation` (`layer_cube_missing` for `layer_of`) |
| identity integers are integer tokens (no `1.0`, `1e3`), at mint and in a full envelope's `fields` | V+TS (TS via `floatTokenPaths`) | `float_in_identity` |
| `honesty.note` is a string of at most 400 chars | S+V+TS | `bad_honesty` |

### status → honesty (C3)

`status` says whether the bytes are here; `honesty` says what the bytes are.
They are independent axes, but these combinations are the only ones writers
produce (migration and `build_assets`), and the UI derives its badge text from
`honesty.claims`, never from `status`:

| kind / source | `status` | `synthesized_speech` | `fixture` | `claims` | UI meaning |
|---|---|---|---|---|---|
| audio_clip, synthesized, wav present | `ok` | true | false | `real_wav`, `synthesized_speech` | playable real synthesis |
| audio_clip, synthesized, wav sha known but file absent here | `missing` | true | false | `real_wav`, `synthesized_speech` | real synthesis, not on this machine |
| audio_clip, not synthesized (recording/import) | `ok` / `missing` | false | false | `real_wav` | real audio, no synthesis claim |
| audio_clip | `unavailable` | — | — | — | **invalid** (`bad_status`): an unavailable engine has no clip; it is a voice_model |
| voice_model, engine present | `ok` | false | false | `[]` or `g2p_only` | engine listed |
| voice_model, engine unavailable | `unavailable` | false | false | `engine_unavailable` (+ `g2p_only`) | greyed engine, never playable |
| voice_profile | `ok` | **false** (enforced) | false | `profile_preview`, `not_a_podcast_render`; `not_podcast:true` (enforced) | persona preview, not a render |
| cube_ihdr / layer | `ok` | false | false | `library_cube` | analysis of a real clip |
| spectrogram_2d | `ok` | false | false | `library_spectrogram` | analysis of a real clip |
| card, fixture tone | `ok` | **false** (enforced) | true | `fixture_tone` | test tone, dev/test only |
| card, status/health views | `ok` | false | false | `status_only` | no audio claim |
| card, benchmark | `ok` | false | false | `reference_only` | reference numbers only |
| card, library clip | `ok` | false | false | `real_wav` (clip ok) / `engine_unavailable` | mirrors the bound clip |

Cards never claim synthesized speech themselves; they point at the clip that
does through `relations.bound_to`.

**Claims vocabulary:** `real_wav, synthesized_speech, library_cube,
library_spectrogram, fixture_tone, profile_preview, reference_only,
not_a_podcast_render, engine_unavailable, g2p_only, status_only, sample_content`.

**Release vs dev (PR #5 review).** An asset is dev/test-only when
`honesty.fixture` is true or it claims `fixture_tone`, `reference_only` or
`sample_content` (Rust `is_dev_fixture`, TS `isDevFixture`): today the
`spec-fixture`, `cube-fixture`, `bench-ref`, `cast-sample` (sample script),
`serve-node` and `serve-gateway` (never-probed placeholder endpoints) cards. `build_assets` writes them to
`schemas/asset-object/fixtures/assets.dev.json` instead of the public
`assets.json`, and writes `viewport.release.json` (the example deck minus
those cards). Release builds boot `viewport.release.json`; only
`VITE_GEN_AUDIO_FIXTURES=1` (PR #4's flag) loads the example deck and the dev
envelopes, through dynamic imports that a release build does not emit.
`fixtures_v1.json` still pins every uid (release and dev).

## 6. Media references

- **Paths** are relative to the library root (`apps/desktop/public/library`),
  such as `name.ext` or `dir/name.ext`, with segments matching
  `[A-Za-z0-9_-][A-Za-z0-9_.-]*`. Each of these is rejected with its own error
  code:
  - empty path
  - `..` anywhere, including inside a segment (`a..b.wav`; schema
    `not: {pattern: "\\.\\."}` on `media_path` and `wav_url`)
  - absolute paths (`/x`)
  - UNC paths (`\\server`, `//server`)
  - drive letters (`C:`)
  - URL schemes (`https:`, `file:`)
  - backslashes
  - dot-files
- **Web URL:** `/library/<path>`. `audio_clip.body.wav_url` follows exactly
  the media-path rules after the `/library/` prefix (up to 4 segments), in the
  schema, Rust and TS (`bad_wav_url`).
- **On load:** `verify_media` checks the size cap (256 MiB), the byte length
  and the sha256.
- **Missing files:** a file absent on this machine (the WAVs are gitignored)
  is reported as `missing` rather than failing. CI verifies every committed
  file.

## 7. MCP / ACP

- **`asset_resolve {uid}`**:
  - Accepts a full uid, or a prefix of at least `ga:<kind>:` plus 8 chars.
  - Returns the envelope plus `glyph`, `hueClass` and `absolutePathsOmitted:true`.
- **`asset_list {kind?, cursor?, limit?}`**:
  - `limit` 1–100, default 50.
  - Ordered by uid. `cursor` is the last uid returned, so the server stays
    stateless.
- **`asset_glyph {uid}`** returns `{glyph, codepoints, dots, hueClass, ariaLabel}`.
- **Errors:**
  - A malformed uid or an unknown kind is JSON-RPC `-32602`.
  - `{ok:false, code:"asset_not_found"}` and
    `{ok:false, code:"asset_ambiguous_prefix", candidates:[…≤10]}` both set
    `isError`.
  - A kind/identity mismatch in stored data is `-32603`.
- **Existing UI tools** keep `tileId`/`clipId` and accept an optional `uid`.
  If both are given, they must name the same asset.
- **`ui_navigate {slide}` (C5)** takes the converged slide id
  `slide:<slug>` (for example `slide:library`, `slide:spatial`). A bare slug
  is a deprecated alias: it still resolves, the MCP server logs a deprecation
  line on stderr, and the result carries `deprecation`. The queued event
  always carries the canonical `slide:<slug>`; the UI accepts both forms.
- **Play origin (C1, Optimus ruling on PR #5).** TS does not decide focus
  and never rebinds the Cube tab or the shared clock on Play. A UI Play click
  (tile or Cube tab) plays locally and posts MCP `ui_playback
  {action:"play", origin:"user"}` on the control bus
  (`apps/desktop/src/play-control.ts`); `origin` is `user` (default) or
  `auto` and applies to `play` only. Autoplay and other programmatic starts
  call `playClip(..., "auto")` and change nothing but the audio. Focus and
  rebind belong to the Rust viewport reducer (PR #4), which is not part of
  this change. The playhead stays transport-local.
- **ACP `session/new`** accepts `assets:[uid]` (at most 8, resolved through
  the same catalog) and stores uids in the track, never paths.

## 8. Reconciled choices (Optimus × Darbot)

| topic | v1 choice |
|---|---|
| kinds | Darbot's snake_case tokens (`voice_profile`, `podcast_script`); `voice_profile` = agent persona |
| identity | fixed per-kind allowlist (Darbot), integers only, ASCII keys, NFC, no controls (Optimus) |
| preimage | `"ga-asset-v1\0" + JCS({kind, schema_major, fields, media[{role,sha256}], src})`; media inside the JSON |
| uid | first 128 bits, base32 lowercase, 26 chars, pad bits 0 (Optimus); `uid_scheme:"ga1"`; `supersedes` |
| glyph | 2 cells from digest bytes 0–1, identity bit→dot mapping, SVG with faint empty dots, kind-class hue |
| schema | hand-written 2020-12 is the source of truth; checked by the jsonschema crate (cargo test) and Ajv2020 (`npm test`); no schemars |
| vectors | `schemas/asset-object/vectors/v1.json` shared by `cargo test` and `npm test` (CI step added) |

## 9. Open items for the hardening review

Darbot's RED review of `c97a69e` (B1, B2, D1–D4, D6, C2, C3) is folded in
above. D6: the `v0_manifest_clip_to_v1` migration vector now uses a real
synthetic file, a 1.000 s 24 kHz mono 16-bit silent PCM WAV (44 + 48000 =
48044 bytes) whose sha256 matches its byte count; it is labelled
`synthetic_media` in the vector.

- The TS side mirrors canonicalization, uid, glyph, uid parsing and media-path
  checks, and runs Ajv over every envelope vector. The v0→v1 migration and the
  set-level invariants are implemented in Rust only. The UI reads the
  pre-migrated `assets.json` and never mints, so a TS migrate would serve only
  parity.
- `mcp_tool` envelopes are defined and covered by vectors, but the MCP server
  does not publish its own tools as assets yet.
- `podcast_script` and `transcript` have schemas but no library instances yet;
  the P4 pipeline will be the first writer.
