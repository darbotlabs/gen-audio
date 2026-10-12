import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

test("shipped schema validators work when dynamic JavaScript compilation is prohibited", () => {
  const script = `
    import { readFileSync } from 'node:fs';
    import { viewportValidator, profileValidator, catalogValidator, semanticCubeValidator } from './src/generated/contract-validators.js';
    const load = p => JSON.parse(readFileSync(p, 'utf8'));
    const viewport = load('../../schemas/examples/viewport.release.json');
    const catalog = load('./public/library/assets.json');
    const profile = load('../../schemas/examples/voice_profile.alice.json');
    for (const doc of load('../../schemas/examples/semantic-cubes.json')) {
      if (!semanticCubeValidator(doc)) throw new Error(JSON.stringify(semanticCubeValidator.errors));
    }
    for (const [validator, data] of [[viewportValidator, viewport], [catalogValidator, catalog], [profileValidator, profile]]) {
      if (!validator(data)) throw new Error(JSON.stringify(validator.errors));
      if (validator({})) throw new Error('Validator accepted an invalid document');
    }
  `;
  const result = spawnSync(process.execPath, ["--disallow-code-generation-from-strings", "--input-type=module", "-e", script],
    { cwd: fileURLToPath(new URL("..", import.meta.url)), encoding: "utf8" });
  assert.equal(result.status, 0, result.stdout + result.stderr);
});
