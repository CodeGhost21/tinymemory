const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const { uploadRequest } = require("./workflows.js");

class FakeFormData {
  constructor() { this.parts = []; }
  append(...part) { this.parts.push(part); }
}

class FakeClassList {
  constructor(value = "") { this.names = new Set(value.split(/\s+/).filter(Boolean)); }
  add(...names) { names.forEach((name) => this.names.add(name)); }
  remove(...names) { names.forEach((name) => this.names.delete(name)); }
  contains(name) { return this.names.has(name); }
  toggle(name, force) {
    const enabled = force === undefined ? !this.contains(name) : force;
    if (enabled) this.add(name); else this.remove(name);
    return enabled;
  }
}

class FakeElement {
  constructor(tagName, attributes) {
    this.tagName = tagName.toUpperCase();
    this.id = attributes.id || "";
    this.type = attributes.type || "";
    this.value = attributes.value || "";
    this.checked = Object.hasOwn(attributes, "checked");
    this.files = [];
    this.dataset = {};
    this.style = {};
    this.textContent = "";
    this.classList = new FakeClassList(attributes.class || "");
    this.listeners = new Map();
    if (attributes["data-op"]) this.dataset.op = attributes["data-op"];
  }

  addEventListener(name, handler) {
    const handlers = this.listeners.get(name) || [];
    handlers.push(handler);
    this.listeners.set(name, handlers);
  }

  async fire(name) {
    const results = (this.listeners.get(name) || []).map((handler) => handler({ target: this }));
    await Promise.all(results);
  }

  async click() { await this.fire("click"); }
}

function parseAttributes(source) {
  const attributes = {};
  for (const match of source.matchAll(/([:\w-]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)) {
    attributes[match[1]] = match[2] ?? match[3] ?? match[4] ?? "";
  }
  return attributes;
}

function parseDocument(html) {
  const elements = [];
  const byId = new Map();
  for (const match of html.matchAll(/<(input|select|textarea|button|span|div|pre|p|label)\b([^>]*)>/gi)) {
    const element = new FakeElement(match[1], parseAttributes(match[2]));
    elements.push(element);
    if (element.id) byId.set(element.id, element);
  }

  for (const match of html.matchAll(/<select\b([^>]*)>([\s\S]*?)<\/select>/gi)) {
    const select = byId.get(parseAttributes(match[1]).id);
    if (!select) continue;
    const options = [...match[2].matchAll(/<option\b([^>]*)>/gi)].map((option) => parseAttributes(option[1]));
    const selected = options.find((option) => Object.hasOwn(option, "selected")) || options[0];
    if (selected) select.value = selected.value || "";
  }

  for (const match of html.matchAll(/<textarea\b([^>]*)>([\s\S]*?)<\/textarea>/gi)) {
    const textarea = byId.get(parseAttributes(match[1]).id);
    if (textarea) textarea.value = match[2];
  }

  return {
    getElementById(id) { return byId.get(id) || null; },
    querySelectorAll(selector) {
      if (selector === "input, select, textarea") {
        return elements.filter((element) => ["INPUT", "SELECT", "TEXTAREA"].includes(element.tagName));
      }
      if (selector.startsWith(".")) {
        return elements.filter((element) => element.classList.contains(selector.slice(1)));
      }
      throw new Error(`unsupported querySelectorAll selector: ${selector}`);
    },
    querySelector(selector) {
      const match = selector.match(/^\.([\w-]+)\[data-op="([\w-]+)"\]$/);
      if (match) {
        return elements.find((element) => element.classList.contains(match[1]) && element.dataset.op === match[2]) || null;
      }
      throw new Error(`unsupported querySelector selector: ${selector}`);
    },
  };
}

function response(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    async text() { return body === null ? "" : JSON.stringify(body); },
  };
}

const ENGINE_LIST = [
  { id: "tinycortex", label: "TinyCortex (local)", description: "In-process.", needs_endpoint: false, needs_key: false, key_optional: false, deployments: [], default_endpoint: null, hosted: false },
  { id: "mem0", label: "Mem0", description: "Mem0.", needs_endpoint: true, needs_key: true, key_optional: true, deployments: ["cloud", "self_hosted"], default_endpoint: "https://api.mem0.ai", hosted: false },
  { id: "cortex", label: "CortexDB", description: "CortexDB.", needs_endpoint: true, needs_key: true, key_optional: false, deployments: ["cloud", "self_hosted"], default_endpoint: "https://api-v1.cortexdb.ai", hosted: false },
  { id: "tinyhumans", label: "CortexDB (via TinyHumans)", description: "Hosted.", needs_endpoint: false, needs_key: false, key_optional: false, deployments: [], default_endpoint: "https://api.tinyhumans.ai", hosted: true },
];

const DISCONNECTED = { connected: false, driver_id: null, engine: null, has_graph: false, has_answer: false };

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
}

async function loadActualPage() {
  const webDirectory = __dirname;
  const html = fs.readFileSync(path.join(webDirectory, "index.html"), "utf8");
  const document = parseDocument(html);
  const requests = [];
  const routes = {
    "/api/status": () => response(200, DISCONNECTED),
    "/api/engines": () => response(200, ENGINE_LIST),
    "/api/documents/upload": () => response(200, { route: "documents", key: "note.txt" }),
  };
  const storage = new Map();
  const context = vm.createContext({
    console,
    document,
    FormData: FakeFormData,
    URLSearchParams,
    localStorage: {
      getItem(key) { return storage.has(key) ? storage.get(key) : null; },
      setItem(key, value) { storage.set(key, String(value)); },
      removeItem(key) { storage.delete(key); },
    },
    async fetch(url, options) {
      requests.push({ url, options });
      const route = routes[url];
      if (!route) throw new Error(`unexpected request: ${url}`);
      return route(options);
    },
  });

  const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)];
  assert.ok(scripts.length > 0, "index.html must contain executable scripts");
  for (const script of scripts) {
    const attributes = parseAttributes(script[1]);
    if (attributes.src) {
      const sourcePath = path.join(webDirectory, attributes.src.replace(/^\//, ""));
      assert.ok(fs.existsSync(sourcePath), `referenced page script is missing: ${attributes.src}`);
      vm.runInContext(fs.readFileSync(sourcePath, "utf8"), context, { filename: sourcePath });
    } else if (script[2].trim()) {
      vm.runInContext(script[2], context, { filename: "index.html:inline-script" });
    }
  }
  await settle();

  return {
    document,
    requests,
    routes,
    storage,
    rejectNextUpload(message) {
      routes["/api/documents/upload"] = () => response(400, { error: message });
    },
  };
}

test("upload request uses the document intake route and multipart fields", () => {
  const file = { name: "guide.md", size: 12 };
  const request = uploadRequest(file, {
    namespace: "manuals",
    category: "custom:guide",
    taint: "external_sync",
  }, () => new FakeFormData());
  assert.equal(request.path, "/documents/upload");
  assert.equal(request.options.method, "POST");
  assert.deepEqual(request.options.body.parts, [
    ["namespace", "manuals"],
    ["key", "guide.md"],
    ["category", "custom:guide"],
    ["taint", "external_sync"],
    ["file", file, "guide.md"],
  ]);
});

test("actual page upload wiring renders success and error responses", async () => {
  const page = await loadActualPage();
  const uploadButton = page.document.getElementById("upload-btn");
  assert.ok(uploadButton, "actual page must contain #upload-btn");
  page.document.getElementById("upload-files").files = [{ name: "note.txt", size: 4 }];
  page.document.getElementById("upload-namespace").value = "notes";

  await uploadButton.click();
  const upload = page.requests.find((request) => request.url === "/api/documents/upload");
  assert.ok(upload, "clicking the actual upload button must call /api/documents/upload");
  assert.equal(upload.options.method, "POST");
  assert.deepEqual(upload.options.body.parts[0], ["namespace", "notes"]);
  assert.match(page.document.getElementById("output").textContent, /"status": "stored"/);

  page.rejectNextUpload("upload rejected");
  await uploadButton.click();
  assert.match(page.document.getElementById("output").textContent, /error: upload rejected/);
});

const active = (page, id) => page.document.getElementById(id).classList.contains("active");

test("the engine picker is built from /api/engines, not hard-coded", async () => {
  const page = await loadActualPage();
  const html = page.document.getElementById("engine").innerHTML;
  for (const engine of ENGINE_LIST) {
    assert.ok(html.includes(`value="${engine.id}"`), `missing option for ${engine.id}`);
    assert.ok(html.includes(engine.label), `missing label ${engine.label}`);
  }
  assert.ok(!html.includes('value="local"'), "the old hard-coded id must be gone");
  assert.equal(page.document.getElementById("engine").value, "tinycortex");
});

test("field visibility follows the engine descriptor", async () => {
  const page = await loadActualPage();
  const engine = page.document.getElementById("engine");
  const pick = async (id) => { engine.value = id; await engine.fire("change"); };

  await pick("tinycortex");
  assert.deepEqual(
    [active(page, "field-deployment"), active(page, "field-endpoint"), active(page, "field-key")],
    [false, false, false],
  );

  await pick("mem0");
  assert.deepEqual(
    [active(page, "field-deployment"), active(page, "field-endpoint"), active(page, "field-key")],
    [true, true, true],
  );
  const options = page.document.getElementById("deployment").innerHTML;
  assert.ok(options.includes('value="cloud"') && options.includes('value="self_hosted"'));

  // CortexDB: cloud defaults its endpoint, self-hosted needs one, so it shows.
  await pick("cortex");
  assert.deepEqual(
    [active(page, "field-deployment"), active(page, "field-endpoint"), active(page, "field-key")],
    [true, true, true],
  );

  await pick("tinyhumans");
  assert.deepEqual(
    [active(page, "field-deployment"), active(page, "field-endpoint"), active(page, "field-key")],
    [false, true, true],
  );
  assert.match(page.document.getElementById("api-key-label").textContent, /Session token or API key \(required\)/);
  assert.equal(page.document.getElementById("endpoint").value, "https://api.tinyhumans.ai");
});

test("the Answer tab appears only when the connected engine can answer", async () => {
  const page = await loadActualPage();
  const tab = page.document.getElementById("tab-answer");
  assert.equal(tab.style.display, "none");

  page.routes["/api/connect"] = () => response(200, {
    connected: true, driver_id: "tinyhumans", engine: "tinyhumans", has_graph: false, has_answer: true,
  });
  page.document.getElementById("engine").value = "tinyhumans";
  await page.document.getElementById("engine").fire("change");
  page.document.getElementById("api-key").value = "tiny_live_x";
  await page.document.getElementById("connect-btn").click();
  assert.equal(tab.style.display, "");
  const connect = page.requests.find((r) => r.url === "/api/connect");
  assert.deepEqual(JSON.parse(connect.options.body), {
    engine: "tinyhumans", deployment: null, endpoint: "https://api.tinyhumans.ai", api_key: "tiny_live_x",
  });

  page.routes["/api/disconnect"] = () => response(200, DISCONNECTED);
  await page.document.getElementById("disconnect-btn").click();
  assert.equal(tab.style.display, "none");
});

test("a 402 shows the insufficient-credits banner and a success hides it", async () => {
  const page = await loadActualPage();
  const banner = page.document.getElementById("credits-banner");
  assert.equal(banner.classList.contains("active"), false);

  page.routes["/api/answer"] = () => response(402, { error: "insufficient credits", code: "USER_INSUFFICIENT_CREDITS" });
  await page.document.getElementById("answer-btn").click();
  assert.equal(banner.classList.contains("active"), true);
  assert.match(page.document.getElementById("output").textContent, /insufficient credits/);
  const answer = page.requests.find((r) => r.url === "/api/answer");
  assert.equal(answer.options.method, "POST");

  page.routes["/api/answer"] = () => response(200, { answer: "ok", citations: [] });
  await page.document.getElementById("answer-btn").click();
  assert.equal(banner.classList.contains("active"), false);
});

test("switch and copy posts the target to /api/migrate and reports the copy", async () => {
  const page = await loadActualPage();
  page.routes["/api/connect"] = () => response(200, {
    connected: true, driver_id: "tinycortex", engine: "tinycortex", has_graph: false, has_answer: false,
  });
  await page.document.getElementById("connect-btn").click();
  assert.ok(active(page, "field-migrate"), "the option is offered once connected");

  page.routes["/api/migrate"] = () => response(200, {
    status: { connected: true, driver_id: "tinyhumans", engine: "tinyhumans", has_graph: false, has_answer: true },
    report: { pages: 1, records: 3, imported: 3, skipped: 0, failed: 0, errors: [] },
  });
  page.document.getElementById("engine").value = "tinyhumans";
  await page.document.getElementById("engine").fire("change");
  page.document.getElementById("api-key").value = "jwt";
  page.document.getElementById("migrate-copy").checked = true;
  await page.document.getElementById("connect-btn").click();

  const migrate = page.requests.find((r) => r.url === "/api/migrate");
  assert.ok(migrate, "migrate endpoint must be called");
  assert.equal(JSON.parse(migrate.options.body).to.engine, "tinyhumans");
  assert.match(page.document.getElementById("connect-msg").textContent, /copied 3 of 3 records/);
  assert.equal(page.document.getElementById("status-badge").textContent, "connected: tinyhumans");
});
