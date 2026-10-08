/** Browser fixture tone and drawings. Not a podcast and not the Python DSP chain. */

import { surfaceUiError } from "./play-control";

export interface FixtureBuffer {
  sampleRate: number;
  samples: Float32Array;
}

export function makeFixture(): FixtureBuffer {
  const sampleRate = 16000;
  const edge = Math.floor(0.08 * sampleRate);
  const body = Math.floor(0.5 * sampleRate);
  const samples = new Float32Array(edge * 2 + body);
  for (let index = 0; index < body; index += 1) {
    const t = index / sampleRate;
    samples[edge + index] =
      0.25 * Math.sin(2 * Math.PI * 40 * t) + 0.35 * Math.sin(2 * Math.PI * 440 * t);
  }
  return { sampleRate, samples };
}

export function previewImprove(input: FixtureBuffer): FixtureBuffer {
  const out = new Float32Array(input.samples.length);
  let previous = 0;
  const alpha = 0.92;
  for (let index = 0; index < input.samples.length; index += 1) {
    const sample = input.samples[index];
    previous = alpha * (previous + sample - (index ? input.samples[index - 1] : 0));
    out[index] = previous;
  }
  let start = 0;
  let end = out.length;
  const peak = out.reduce((max, sample) => Math.max(max, Math.abs(sample)), 0);
  const gate = peak * 0.01;
  while (start < end && Math.abs(out[start]) < gate) start += 1;
  while (end > start && Math.abs(out[end - 1]) < gate) end -= 1;
  const trimmed = out.slice(start, end);
  const trimmedPeak = trimmed.reduce((max, sample) => Math.max(max, Math.abs(sample)), 0) || 1;
  const gain = 0.89 / trimmedPeak;
  for (let index = 0; index < trimmed.length; index += 1) trimmed[index] *= gain;
  return { sampleRate: input.sampleRate, samples: trimmed };
}

export function drawSpectrogram(canvas: HTMLCanvasElement, audio: FixtureBuffer, title: string): void {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const width = 480;
  const height = 180;
  canvas.width = width;
  canvas.height = height;
  ctx.fillStyle = "#0a0c10";
  ctx.fillRect(0, 0, width, height);
  const nfft = 256;
  const hop = 128;
  const columns = Math.max(1, Math.floor((audio.samples.length - nfft) / hop));
  for (let col = 0; col < columns; col += 1) {
    const start = col * hop;
    const mags = new Array<number>(nfft / 2).fill(0);
    for (let bin = 1; bin < nfft / 2; bin += 1) {
      let real = 0;
      let imag = 0;
      const freq = (2 * Math.PI * bin) / nfft;
      for (let n = 0; n < nfft; n += 1) {
        const sample = audio.samples[start + n] ?? 0;
        real += sample * Math.cos(freq * n);
        imag -= sample * Math.sin(freq * n);
      }
      mags[bin] = Math.hypot(real, imag);
    }
    const max = Math.max(...mags, 1e-6);
    for (let bin = 1; bin < mags.length; bin += 1) {
      const db = 20 * Math.log10(mags[bin] / max + 1e-8);
      const clamped = Math.min(0, Math.max(-90, db));
      const shade = (clamped + 90) / 70;
      ctx.fillStyle = `rgb(${Math.floor(20 + shade * 40)}, ${Math.floor(30 + shade * 180)}, ${Math.floor(40 + shade * 160)})`;
      const y = height - (bin / mags.length) * (height - 16);
      ctx.fillRect((col / columns) * width, y, width / columns + 1, 3);
    }
  }
  ctx.fillStyle = "#9aa6bd";
  ctx.font = "12px sans-serif";
  ctx.fillText(title, 8, 14);
  canvas.dataset.caption = title;
}

export function drawCube(canvas: HTMLCanvasElement, audio: FixtureBuffer): void {
  const gl = canvas.getContext("webgl");
  if (!gl) {
    const ctx = canvas.getContext("2d");
    ctx?.fillText("WebGL is unavailable in this view.", 12, 24);
    return;
  }
  canvas.width = 480;
  canvas.height = 180;
  gl.viewport(0, 0, canvas.width, canvas.height);
  const points: number[] = [];
  const colors: number[] = [];
  const nfft = 128;
  const hop = 256;
  const columns = Math.min(48, Math.floor(audio.samples.length / hop));
  for (let col = 0; col < columns; col += 1) {
    for (let bin = 1; bin < 24; bin += 1) {
      let real = 0;
      let imag = 0;
      const freq = (2 * Math.PI * bin) / nfft;
      for (let n = 0; n < nfft; n += 1) {
        const sample = audio.samples[col * hop + n] ?? 0;
        real += sample * Math.cos(freq * n);
        imag -= sample * Math.sin(freq * n);
      }
      const mag = Math.hypot(real, imag);
      const lifted = Math.pow(Math.min(mag, 8) / 8, 0.6);
      points.push((col / columns) * 2 - 1, (bin / 24) * 2 - 1, lifted);
      colors.push(0.2, 0.7 + lifted * 0.3, 0.75, 1);
    }
  }
  const vs = `
    attribute vec3 a_pos;
    attribute vec4 a_color;
    uniform float u_angle;
    varying vec4 v_color;
    void main() {
      float c = cos(u_angle);
      float s = sin(u_angle);
      float x = a_pos.x * c - a_pos.z * s;
      float z = a_pos.x * s + a_pos.z * c;
      gl_Position = vec4(x * 0.85, a_pos.y * 0.75, 0.0, 1.4 + z);
      gl_PointSize = 3.0;
      v_color = a_color;
    }
  `;
  const fs = `
    precision mediump float;
    varying vec4 v_color;
    void main() { gl_FragColor = v_color; }
  `;
  const program = link(gl, vs, fs);
  gl.useProgram(program);
  bind(gl, program, "a_pos", 3, new Float32Array(points));
  bind(gl, program, "a_color", 4, new Float32Array(colors));
  const angle = gl.getUniformLocation(program, "u_angle");
  gl.uniform1f(angle, 0.6);
  gl.clearColor(0.04, 0.05, 0.07, 1);
  gl.clear(gl.COLOR_BUFFER_BIT);
  gl.drawArrays(gl.POINTS, 0, points.length / 3);
}

export function play(audio: FixtureBuffer): void {
  const context = new AudioContext();
  const buffer = context.createBuffer(1, audio.samples.length, audio.sampleRate);
  buffer.copyToChannel(audio.samples, 0);
  const source = context.createBufferSource();
  source.buffer = buffer;
  source.connect(context.destination);
  source.start();
  source.onended = () => {
    void context.close().catch((error: unknown) => surfaceUiError(error, "fixture playback"));
  };
}

function link(gl: WebGLRenderingContext, vsSource: string, fsSource: string): WebGLProgram {
  const program = gl.createProgram();
  if (!program) throw new Error("webgl program");
  gl.attachShader(program, shader(gl, gl.VERTEX_SHADER, vsSource));
  gl.attachShader(program, shader(gl, gl.FRAGMENT_SHADER, fsSource));
  gl.linkProgram(program);
  return program;
}

function shader(gl: WebGLRenderingContext, type: number, source: string): WebGLShader {
  const sh = gl.createShader(type);
  if (!sh) throw new Error("shader");
  gl.shaderSource(sh, source);
  gl.compileShader(sh);
  return sh;
}

function bind(gl: WebGLRenderingContext, program: WebGLProgram, name: string, size: number, data: Float32Array): void {
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(program, name);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
}
