import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, realpathSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { checkMediaPath } from "../src/asset.ts";

const library = fileURLToPath(new URL("../public/library/", import.meta.url));
const load = (name: string) => JSON.parse(readFileSync(path.join(library, name), "utf8"));
const assets = load("assets.json").assets;
const manifest = load("manifest.json");
const viewport = JSON.parse(readFileSync(new URL("../../../schemas/examples/viewport.example.json", import.meta.url), "utf8"));

test("packaged library contains assets, not build scripts or execution debris", () => {
  const executableOrOutput = /\.(?:ps1|psm1|psd1|py|pyc|sh|bat|cmd|exe|dll|log|pid|tmp|bak|orig|rej)$/i;
  const generatedDirectories = new Set(["node_modules", "target", "dist", "__pycache__", ".pytest_cache"]);
  const inspect = (directory: string): void => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const file = path.join(directory, entry.name);
      const relative = path.relative(library, file);
      assert.ok(!entry.isSymbolicLink(), `Library asset must not be a symlink: ${relative}`);
      if (entry.isDirectory()) {
        assert.ok(!generatedDirectories.has(entry.name.toLowerCase()), `Generated directory in packaged library: ${relative}`);
        inspect(file);
      } else {
        assert.ok(entry.isFile() && !executableOrOutput.test(entry.name), `Move non-asset output out of packaged library: ${relative}`);
      }
    }
  };
  inspect(library);
});

function mediaBytes(name: string): Buffer {
  checkMediaPath(name);
  const file = realpathSync(path.join(library, name));
  const relative = path.relative(realpathSync(library), file);
  assert.ok(relative && !relative.startsWith("..") && !path.isAbsolute(relative), `Media escapes library: ${name}`);
  return readFileSync(file);
}

function wavInfo(bytes: Buffer) {
  assert.equal(bytes.toString("ascii", 0, 4), "RIFF");
  assert.equal(bytes.toString("ascii", 8, 12), "WAVE");
  assert.ok(bytes.readUInt32LE(4) + 8 <= bytes.length, "Truncated WAV");
  let format = 0, channels = 0, rate = 0, align = 0, bits = 0, data: Buffer | undefined;
  for (let offset = 12; offset + 8 <= bytes.length;) {
    const size = bytes.readUInt32LE(offset + 4);
    const tag = bytes.toString("ascii", offset, offset + 4);
    const start = offset + 8;
    assert.ok(start + size <= bytes.length, `Truncated ${tag} chunk`);
    if (tag === "fmt ") {
      assert.ok(size >= 16);
      format = bytes.readUInt16LE(start);
      channels = bytes.readUInt16LE(start + 2);
      rate = bytes.readUInt32LE(start + 4);
      align = bytes.readUInt16LE(start + 12);
      bits = bytes.readUInt16LE(start + 14);
    } else if (tag === "data") {
      data = bytes.subarray(start, start + size);
    }
    offset = start + size + (size % 2);
  }
  assert.ok(data?.length, "WAV must contain audio");
  assert.ok(channels > 0 && rate > 0 && align === channels * bits / 8);
  assert.equal(data.length % align, 0);
  assert.ok((format === 1 && bits === 16) || (format === 3 && bits === 32), "Unsupported curated WAV encoding");
  let peak = 0;
  for (let offset = 0; offset < data.length; offset += bits / 8) {
    const sample = format === 3 ? data.readFloatLE(offset) : data.readInt16LE(offset) / 32768;
    assert.ok(Number.isFinite(sample), "Non-finite WAV samples");
    peak = Math.max(peak, Math.abs(sample));
  }
  assert.ok(peak > 0, "Claimed speech is silent");
  return { channels, rate, duration: data.length / align / rate };
}

test("all catalog media exists and matches its declared hash and size", () => {
  for (const asset of assets) {
    for (const media of asset.media) {
      const bytes = mediaBytes(media.path);
      assert.equal(bytes.length, media.bytes, `${media.path}: size`);
      assert.equal(createHash("sha256").update(bytes).digest("hex"), media.sha256, `${media.path}: sha256`);
      if (media.role === "wav") {
        const info = wavInfo(bytes);
        assert.equal(info.rate, asset.fields.sample_rate_hz, `${media.path}: rate`);
        assert.equal(info.channels, asset.fields.channels, `${media.path}: channels`);
        assert.ok(Math.abs(info.duration * 1000 - asset.fields.duration_ms) <= 1, `${media.path}: duration`);
      }
    }
  }
});

test("manifest and viewport cannot claim unpackaged or mismatched speech", () => {
  const claimed = [...manifest.clips, ...viewport.cards.filter((card) => card.kind === "LibraryClip").map((card) => ({ id: card.id, ...card.body }))];
  for (const clip of claimed) {
    assert.equal(clip.absWav, undefined, `${clip.id}: machine-local metadata`);
    if (clip.synthesizedSpeech !== true) {
      assert.ok(!clip.wavUrl, `${clip.id}: unavailable clip advertises a WAV`);
      continue;
    }
    assert.ok(clip.wavUrl.startsWith("/library/"), `${clip.id}: packaged URL`);
    const name = clip.wavUrl.slice("/library/".length);
    const asset = assets.find((item) => item.kind === "audio_clip" && item.legacy_id === clip.id);
    assert.ok(asset, `${clip.id}: missing canonical audio asset`);
    assert.ok(asset.media.some((media) => media.role === "wav" && media.path === name), `${clip.id}: catalog URL drift`);
    const info = wavInfo(mediaBytes(name));
    assert.ok(Math.abs(info.duration - clip.duration_s) <= 0.001, `${clip.id}: duration`);
    assert.equal(info.rate, clip.sample_rate, `${clip.id}: sample rate`);
    if (clip.wav) assert.equal(clip.wav, name, `${clip.id}: artifact directory dependency`);
  }
});
