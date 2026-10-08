// main.ts used to be checked by matching source text, so an early return above
// the calls still passed. This loads the module and runs the three paths.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer, type ViteDevServer } from "vite";

type El = {
  tagName: string;
  id: string;
  className: string;
  textContent: string;
  value: string;
  type: string;
  hidden: boolean;
  disabled: boolean;
  dataset: Record<string, string>;
  style: { setProperty: (name: string, value: string) => void };
  children: El[];
  parent: El | null;
  classList: { add: () => void; remove: () => void; toggle: () => boolean; contains: () => boolean };
  append: (...nodes: El[]) => void;
  appendChild: (node: El) => El;
  prepend: (...nodes: El[]) => void;
  replaceChildren: (...nodes: El[]) => void;
  setAttribute: (key: string, value: string) => void;
  getAttribute: (key: string) => string | null;
  addEventListener: () => void;
  removeEventListener: () => void;
  querySelector: (sel: string) => El | null;
  querySelectorAll: (sel: string) => El[];
  closest: (sel: string) => El | null;
  setPointerCapture: () => void;
  focus: () => void;
  matches: (sel: string) => boolean;
};

function matches(el: El, sel: string): boolean {
  const attr = /^\[data-([A-Za-z0-9-]+)(?:="([^"]*)")?\]$/.exec(sel);
  if (attr) {
    const value = el.dataset[attr[1]];
    return attr[2] === undefined ? value !== undefined : value === attr[2];
  }
  if (sel.startsWith("#") && !sel.includes(" ") && !sel.includes(".")) return el.id === sel.slice(1);
  if (sel.startsWith(".")) return el.className.split(/\s+/).includes(sel.slice(1));
  return el.tagName.toLowerCase() === sel.toLowerCase();
}

function walk(el: El, visit: (node: El) => void): void {
  visit(el);
  for (const child of el.children) walk(child, visit);
}

function queryAll(root: El, sel: string): El[] {
  const parts = sel.trim().split(/\s+/);
  let current = [root];
  for (const part of parts) {
    const next: El[] = [];
    for (const node of current) {
      const pool: El[] = [];
      walk(node, (item) => {
        if (item !== node) pool.push(item);
      });
      for (const item of pool) if (matches(item, part)) next.push(item);
    }
    current = next;
  }
  return current;
}

function query(root: El, sel: string): El | null {
  if (sel.startsWith("#") && !sel.includes(" ") && !sel.includes("[") && !sel.includes(".")) {
    let found: El | null = null;
    walk(root, (node) => {
      if (node.id === sel.slice(1)) found = node;
    });
    return found;
  }
  return queryAll(root, sel)[0] ?? null;
}

function makeEl(tag = "div"): El {
  const style: Record<string, string> = {};
  const el = {
    tagName: tag.toUpperCase(),
    id: "",
    className: "",
    textContent: "",
    value: "",
    type: "",
    hidden: false,
    disabled: false,
    selected: false,
    dataset: {} as Record<string, string>,
    style: {
      setProperty(name: string, value: string) {
        style[name] = value;
      },
    },
    children: [] as El[],
    parent: null as El | null,
    classList: {
      add() {},
      remove() {},
      toggle() {
        return false;
      },
      contains() {
        return false;
      },
    },
    append(...nodes: El[]) {
      for (const node of nodes) {
        node.parent = el as El;
        el.children.push(node);
      }
    },
    appendChild(node: El) {
      el.append(node);
      return node;
    },
    prepend(...nodes: El[]) {
      el.children.unshift(...nodes);
    },
    replaceChildren(...nodes: El[]) {
      el.children = [];
      el.append(...nodes);
    },
    setAttribute() {},
    getAttribute() {
      return null;
    },
    addEventListener() {},
    removeEventListener() {},
    querySelector(sel: string) {
      return query(el as El, sel);
    },
    querySelectorAll(sel: string) {
      return queryAll(el as El, sel);
    },
    closest(sel: string) {
      let node: El | null = el as El;
      while (node) {
        if (matches(node, sel)) return node;
        node = node.parent;
      }
      return null;
    },
    setPointerCapture() {},
    focus() {},
    matches(sel: string) {
      return matches(el as El, sel);
    },
  };
  return new Proxy(el, {
    get(target, key, receiver) {
      if (Reflect.has(target, key)) return Reflect.get(target, key, receiver);
      return () => receiver;
    },
  }) as El;
}

function installDom(): { root: El; byId: Map<string, El> } {
  const byId = new Map<string, El>();
  const body = makeEl("body");
  const root = makeEl("html");
  root.append(body);
  const ids = [
    "board",
    "empty",
    "status",
    "model-drop",
    "loaded-model",
    "loaded-model-id",
    "prompt",
    "source-files",
    "file-list",
    "voice",
    "agents",
    "add-agent",
    "duration",
    "generate-podcast",
    "show-empty",
    "layer-switch",
    "floating-playback",
  ];
  for (const id of ids) {
    const tag = id === "voice" || id === "duration" ? "select" : id === "prompt" ? "textarea" : id === "source-files" ? "input" : "div";
    const el = makeEl(tag);
    el.id = id;
    byId.set(id, el);
    body.append(el);
  }
  const documentMock = {
    documentElement: root,
    body,
    querySelector(sel: string) {
      if (sel.startsWith("#") && !sel.includes(" ")) return byId.get(sel.slice(1)) ?? query(body, sel);
      return query(body, sel);
    },
    querySelectorAll(sel: string) {
      return queryAll(body, sel);
    },
    createElement: (tag: string) => makeEl(tag),
    createElementNS: (_ns: string, tag: string) => makeEl(tag),
    addEventListener() {},
  };
  const windowMock = {
    document: documentMock,
    addEventListener() {},
    requestAnimationFrame() {
      return 1;
    },
    cancelAnimationFrame() {},
    setTimeout,
    clearTimeout,
    setInterval() {
      return 0;
    },
    clearInterval() {},
    matchMedia: () => ({ matches: false, addEventListener() {}, addListener() {} }),
    devicePixelRatio: 1,
    location: { href: "http://127.0.0.1/" },
  };
  Object.assign(globalThis, {
    document: documentMock,
    window: windowMock,
    CSS: { escape: (value: string) => value },
    requestAnimationFrame() {
      return 1;
    },
    cancelAnimationFrame() {},
  });
  return { root, byId };
}

test("main.ts counts a failed lookup, a failed library fetch, and a dropped resolve", async () => {
  const { root, byId } = installDom();
  const calls: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push(`${init?.method ?? "GET"} ${url}`);
    if ((init?.method ?? "GET") === "POST") {
      const body = JSON.stringify({ ok: false, error: "missing uid" });
      return new Response(JSON.stringify({ result: { content: [{ text: body }] } }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }
    return new Response("no", { status: 404 });
  }) as typeof fetch;

  let server: ViteDevServer | undefined;
  try {
    server = await createServer({
      root: new URL("..", import.meta.url).pathname,
      configFile: new URL("../vite.config.ts", import.meta.url).pathname,
      server: { middlewareMode: true },
      appType: "custom",
      logLevel: "error",
    });
    const main = (await server.ssrLoadModule("/src/main.ts")) as {
      discoverMcp: () => Promise<string>;
      mediaBlob: (urlPath: string) => Promise<string | null>;
      insertGeneratedTile: (uid: string, args: Record<string, unknown>) => Promise<void>;
    };
    await new Promise((resolve) => setTimeout(resolve, 50));
    const booted = JSON.parse(root.dataset.mcpFailures ?? "{}") as Record<string, number>;
    assert.equal(booted["mcp_status:TypeError"], 1, "boot lookup must count the tauri failure");

    const origin = await main.discoverMcp();
    assert.equal(origin, "http://127.0.0.1:8765");
    assert.match(byId.get("status")?.textContent ?? "", /127\.0\.0\.1:8765/);
    assert.match(byId.get("status")?.textContent ?? "", /TypeError/);

    const blob = await main.mediaBlob("/library/x.wav");
    assert.equal(blob, null);
    const afterFetch = JSON.parse(root.dataset.mcpFailures ?? "{}") as Record<string, number>;
    assert.equal(afterFetch["library_media:http_404"], 1);
    assert.ok(calls.some((line) => line.includes("/library/x.wav")), calls.join("\n"));

    const uid = "ga:audio_clip:abc";
    await main.insertGeneratedTile(uid, {});
    const slide = byId.get("board")?.querySelector('[data-slide="library"]');
    const tile = slide?.querySelector(`[data-uid="${uid}"]`);
    assert.ok(tile, "a failed resolve still paints a tile");
    assert.equal(tile?.dataset.resolve, "failed");
    const error = tile?.querySelector(".resolve-error");
    assert.match(error?.textContent ?? "", new RegExp(uid));
    assert.match(error?.textContent ?? "", /missing uid/);
    const afterResolve = JSON.parse(root.dataset.mcpFailures ?? "{}") as Record<string, number>;
    assert.equal(afterResolve["asset_resolve:ok_false"], 1);
  } finally {
    await server?.close();
  }
});
