import { test } from "node:test";
import assert from "node:assert/strict";
import examples from "../../../schemas/examples/semantic-cubes.json";
import { axisValue, hexRgb, parseSemanticCube, projectSemanticCube } from "../src/semantic-cube";

test("all five domains render their actual metric observations and retain the source", () => {
  for (const input of examples) {
    const { document, projection } = projectSemanticCube(input);
    assert.deepEqual(document, input);
    assert.notEqual(document, input);
    assert.equal(projection.points.length, input.records.length * input.layers.length);
    assert.equal(projection.skipped, 0); assert.equal(projection.clipped, 0);
    assert.ok(projection.points.every(point => [point.t,point.f,point.z,point.v].every(value => value >= 0 && value <= 1)));
  }
});

test("rejects broken mappings, invalid scales, units, IDs, colors and timestamps", () => {
  const mutations: Array<(doc: any) => void> = [
    doc => doc.layers[0].maximum = 0,
    doc => doc.layers[0].scale = "log",
    doc => doc.layers[0].unit = "USD",
    doc => doc.layers[0].metric = "absent",
    doc => doc.layers[0].color = "url(file:///secret)",
    doc => doc.layers[0].color = "#abcd",
    doc => doc.layers[1].id = doc.layers[0].id,
    doc => doc.layers[1].position = doc.layers[0].position,
    doc => doc.records[1].id = doc.records[0].id,
    doc => doc.records[0].x = -1,
    doc => doc.records[0].metrics.confidence = 2,
    doc => doc.records[0].metrics.confidence = NaN,
    doc => doc.records[0].context.session_id = "bad\ncontext",
    doc => doc.axes.x.maximum = Infinity,
    doc => doc.script = "alert(1)",
  ];
  for (const mutate of mutations) { const doc = structuredClone(examples[0]); mutate(doc); assert.throws(() => projectSemanticCube(doc)); }
  const market = structuredClone(examples[3]) as any;
  market.records[0].metrics.low = 103; assert.throws(() => projectSemanticCube(market), /OHLC/);
  market.records[0].metrics.low = 98;
  market.records[0].context.timestamp = "2023-02-30T22:13:20Z"; assert.throws(() => projectSemanticCube(market), /timestamp/);
});

test("zero remains observed, null is missing, clipping is explicit and logarithms are supported", () => {
  const doc = structuredClone(examples[0]) as any;
  doc.records[0].metrics.latency_ms = null; doc.records[0].metrics.tokens = 0;
  const { projection } = projectSemanticCube(doc);
  assert.equal(projection.skipped, 1); assert.equal(projection.points[0].v, 0);
  doc.records[1].metrics.latency_ms = 3000;
  assert.throws(() => projectSemanticCube(doc), /outside/);
  doc.layers[0].outside = "clip"; assert.equal(projectSemanticCube(doc).projection.clipped, 1);
  doc.layers[0].scale = "log"; doc.layers[0].minimum = 1;
  assert.equal(projectSemanticCube(doc).projection.clipped, 1);
});

test("custom metrics avoid inherited object properties and stay bounded", () => {
  const doc = structuredClone(examples[0]) as any;
  doc.domain = "custom"; doc.layers = [doc.layers[0]]; doc.layers[0].metric = "constructor";
  doc.records = doc.records.slice(0, 2); doc.records[0].metrics = { constructor: 5 };
  doc.records[1].metrics = { different: 10 };
  const { projection } = projectSemanticCube(doc); assert.equal(projection.skipped, 1); assert.equal(projection.points.length, 1);
  assert.throws(() => parseSemanticCube(" ".repeat(1024*1024+1)), /1 MiB/);
  assert.throws(() => parseSemanticCube('{"kind":'), SyntaxError);
  assert.deepEqual(hexRgb("#ff0080"), [1,0,128/255]);
  assert.equal(axisValue({ label:"Time", unit:"unix_s", minimum:1700000000, maximum:1700000300 },0),"22:13:20");
});

test("logarithms preserve adjacent large values and subnormals", () => {
  for (const [minimum, maximum] of [[1e15-0.125,1e15],[Number.MIN_VALUE,1e15]]) {
    const doc = structuredClone(examples[0]) as any;
    doc.domain = "custom"; doc.layers = [doc.layers[0]]; doc.records = doc.records.slice(0,2);
    Object.assign(doc.layers[0], { minimum, maximum, scale: "log" });
    doc.records[0].metrics.latency_ms = minimum; doc.records[1].metrics.latency_ms = maximum;
    const { projection } = projectSemanticCube(doc);
    assert.deepEqual(projection.points.map(point => point.v), [0,1]);
  }
});
