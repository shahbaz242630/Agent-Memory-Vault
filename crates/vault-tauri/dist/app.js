// Memory Vault — V0.2 beta UI logic ("Quiet" direction).
// Vanilla JS, no framework, no eval, no remote resources (CSP: 'self').
// All user/vault content rendered through esc() — BRD §11.12 vault-tauri
// checklist (XSS prevention in webview).

// Guard the bridge so the UI still renders (with failing commands) when
// opened outside Tauri — e.g. design review in a plain browser.
const invoke = window.__TAURI__ && window.__TAURI__.core
  ? window.__TAURI__.core.invoke
  : async () => { throw new Error("vault engine not connected (running outside Tauri)"); };

// ---------------------------------------------------------------- utilities

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

function $(id) { return document.getElementById(id); }

function relTime(iso) {
  const t = typeof iso === "number" ? iso : Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 604800) return `${Math.floor(s / 86400)}d ago`;
  return new Date(t).toLocaleDateString();
}

const store = {
  get(key, fallback) {
    try {
      const v = localStorage.getItem(key);
      return v === null ? fallback : JSON.parse(v);
    } catch { return fallback; }
  },
  set(key, val) {
    try { localStorage.setItem(key, JSON.stringify(val)); } catch { /* non-fatal */ }
  },
};

async function copyText(text, btn) {
  let ok = false;
  try {
    await navigator.clipboard.writeText(text);
    ok = true;
  } catch {
    // WebView2 fallback: hidden textarea + execCommand.
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      ok = document.execCommand("copy");
      ta.remove();
    } catch { ok = false; }
  }
  if (btn) {
    btn.textContent = ok ? "copied ✓" : "select + copy manually";
    setTimeout(() => { btn.textContent = "copy"; }, 1600);
  }
}

// ---------------------------------------------------------------- app state

// Every fact shown in onboarding is real: the engine performed these steps
// during Tauri setup() before this webview loaded (main.rs steps 1-7) — the
// welcome animation replays them, it does not fake them.
const CHECK_DEFS = [
  {
    phases: ["locating vault store", "deriving key — Credential Manager", "unsealing store — AES-256"],
    done: "vault unsealed — AES-256, key in Windows Credential Manager",
  },
  // White-label rule (founder, 2026-07-11): never name the underlying
  // models or stack in the UI — the user-facing promise is "on-device".
  {
    phases: ["waking the recall engine", "loading on-device intelligence", "indexing your memory space"],
    done: "recall engine ready — runs entirely on this device",
  },
  {
    phases: ["attaching audit log", "opening default boundary"],
    done: "audit log active — every read & write recorded",
  },
];
const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const CHECK_MS = 2800; // per-check animation duration — one line completes fully before the next appears

// MCP connection snippets use the REAL entry point: `vault-cli mcp serve`
// (stdio, 1:1) — cross-agent proven (Claude / Cursor / Codex).
const SNIPPET_JSON = `{
  "mcpServers": {
    "memory-vault": {
      "command": "vault-cli",
      "args": ["mcp", "serve"]
    }
  }
}`;
const SNIPPET_TOML = `[mcp_servers.memory_vault]
command = "vault-cli"
args = ["mcp", "serve"]`;

const AGENTS = [
  { name: "Claude Code", desc: "CLI agent, connects over stdio", hint: "Add this to your MCP settings, then restart Claude Code:", snippet: SNIPPET_JSON },
  { name: "Claude Desktop", desc: "Edit its MCP config file", hint: "Add this to claude_desktop_config.json:", snippet: SNIPPET_JSON },
  { name: "Codex", desc: "OpenAI's coding agent", hint: "Add this to ~/.codex/config.toml as an MCP server:", snippet: SNIPPET_TOML },
  { name: "Cursor", desc: "AI code editor with MCP support", hint: "Add this to Cursor's MCP settings (mcp.json):", snippet: SNIPPET_JSON },
  { name: "Antigravity", desc: "Google's agentic IDE", hint: "Add this to Antigravity's MCP config:", snippet: SNIPPET_JSON },
  { name: "Custom client", desc: "Any MCP-compatible agent", hint: "Point your client at the vault's stdio server:", snippet: SNIPPET_JSON },
];

const state = {
  screen: store.get("mv_onboarded", false) ? "home" : "welcome",
  checksDone: 0,
  agentPicked: null,
  memType: "semantic",
  addType: "semantic",
  tab: "memories",
  query: "",
  recent: store.get("mv_recent", []),        // memories added via this UI
  agents: store.get("mv_agents", []),        // agents configured via this UI
  showConnectPanel: false,
  showInlineAdd: false,
  toastTimer: null,
  searchSeq: 0,
};

// ---------------------------------------------------------------- screens

function showScreen(name) {
  state.screen = name;
  for (const s of ["welcome", "connect", "memory", "home"]) {
    $(`screen-${s}`).classList.toggle("hidden", s !== name);
  }
  const steps = ["welcome", "connect", "memory"];
  const idx = steps.indexOf(name);
  $("progress-dots").classList.toggle("hidden", idx === -1);
  if (idx !== -1) {
    [...$("progress-dots").children].forEach((el, i) => el.classList.toggle("on", i <= idx));
  }
  if (name === "welcome") runChecks();
  if (name === "home") renderHome();
}

// -- welcome boot checks ----------------------------------------------------

// One row at a time (founder feedback 2026-07-11): a line appears, spins
// through its phases, resolves to [✓] — only then does the next line
// appear. Rows are created once and updated in place so their entrance
// fade never restarts.
let checkTimers = [];
function runChecks() {
  checkTimers.forEach(clearTimeout);
  checkTimers = [];
  state.checksDone = 0;
  const box = $("checks");
  box.innerHTML = "";
  renderBeginState();

  const rows = [];
  let frame = 0;
  let rowStartFrame = 0;

  function addRow() {
    const row = document.createElement("div");
    row.className = "check-item active";
    row.innerHTML = '<span class="ico"></span><span class="lbl"></span>';
    box.appendChild(row);
    rows.push(row);
    rowStartFrame = frame;
    updateActiveRow();
  }

  function updateActiveRow() {
    const i = state.checksDone;
    if (i >= CHECK_DEFS.length) return;
    const def = CHECK_DEFS[i];
    const row = rows[i];
    if (!row) return; // spinner tick before this row's entrance delay
    const local = frame - rowStartFrame;
    const framesPerPhase = Math.max(2, Math.floor(CHECK_MS / 90 / def.phases.length));
    const phase = def.phases[Math.min(def.phases.length - 1, Math.floor(local / framesPerPhase))];
    const dots = ".".repeat(1 + (Math.floor(local / 4) % 3));
    row.querySelector(".ico").textContent = ` ${SPIN[local % SPIN.length]} `;
    row.querySelector(".lbl").textContent = phase + dots;
  }

  function markDone(i) {
    const row = rows[i];
    row.classList.remove("active");
    row.classList.add("done");
    row.querySelector(".ico").textContent = "[✓]";
    row.querySelector(".lbl").textContent = CHECK_DEFS[i].done;
  }

  const spinner = setInterval(() => {
    if (state.screen !== "welcome" || state.checksDone >= CHECK_DEFS.length) {
      clearInterval(spinner);
      return;
    }
    frame += 1;
    updateActiveRow();
  }, 90);
  checkTimers.push(spinner);

  checkTimers.push(setTimeout(addRow, 400));
  for (let i = 1; i <= CHECK_DEFS.length; i++) {
    checkTimers.push(setTimeout(() => {
      markDone(i - 1);
      state.checksDone = i;
      if (i < CHECK_DEFS.length) {
        addRow();
      } else {
        renderBeginState();
      }
    }, 400 + CHECK_MS * i));
  }
}

function renderBeginState() {
  const ready = state.checksDone >= CHECK_DEFS.length;
  // The Begin button stays invisible (space reserved) until every check
  // has finished, then fades in (founder feedback 2026-07-11).
  $("begin-btn").classList.toggle("reveal", ready);
  $("begin-hint").textContent = ready
    ? "Three gentle steps — about two minutes."
    : "Establishing your vault — nothing leaves this device…";
}

// -- connect agent ----------------------------------------------------------

function renderAgentCards() {
  $("agent-grid").innerHTML = AGENTS.map((a, i) => `
    <div class="agent-card${state.agentPicked === i ? " picked" : ""}" data-i="${i}">
      <div class="nm">${esc(a.name)}</div>
      <div class="ds">${esc(a.desc)}</div>
    </div>`).join("");
  const picked = state.agentPicked;
  $("snippet-wrap").classList.toggle("hidden", picked === null);
  if (picked !== null) {
    $("snippet-hint").textContent = AGENTS[picked].hint;
    $("snippet-code").textContent = AGENTS[picked].snippet;
    $("connect-cta").textContent = "I've added it — continue";
  } else {
    $("connect-cta").textContent = "Continue";
  }
}

function connectContinue() {
  if (state.agentPicked !== null) {
    const name = AGENTS[state.agentPicked].name;
    if (!state.agents.some((a) => a.name === name)) {
      state.agents.push({ name, transport: "mcp · stdio", when: Date.now() });
      store.set("mv_agents", state.agents);
    }
  }
  showScreen("memory");
}

// -- first memory -----------------------------------------------------------

// The engine's memory_type taxonomy (semantic / episodic / procedural) is
// write-side metadata — it never gates recall. The UI speaks plain English
// and maps to the backend values; onboarding doesn't ask at all (founder
// decision 2026-07-11).
const TYPE_OPTIONS = [
  { value: "semantic", label: "a fact about me" },
  { value: "episodic", label: "something that happened" },
  { value: "procedural", label: "how I do things" },
];
const TYPE_ROW_LABEL = { semantic: "fact", episodic: "event", procedural: "how-to" };

function renderTypeChips(containerId, current, onPick) {
  $(containerId).innerHTML = TYPE_OPTIONS.map((t) =>
    `<button class="type-chip${current === t.value ? " on" : ""}" data-t="${t.value}">${t.label}</button>`).join("");
  [...$(containerId).querySelectorAll(".type-chip")].forEach((el) => {
    el.addEventListener("click", () => onPick(el.dataset.t));
  });
}

function renderMemoryScreen() {
  const has = $("mem-text").value.trim().length > 0;
  $("mem-save").classList.toggle("disabled", !has);
}

async function saveFirstMemory() {
  const text = $("mem-text").value.trim();
  if (!text) return;
  $("mem-save").classList.add("disabled");
  $("mem-err").textContent = "";
  try {
    const id = await invoke("add_memory", {
      content: text,
      memoryType: state.memType,
      boundary: "default",
    });
    rememberLocally(id, state.memType, text);
    finishOnboarding(true);
  } catch (err) {
    $("mem-err").textContent = `Couldn't keep that memory: ${err}`;
    $("mem-save").classList.remove("disabled");
  }
}

function finishOnboarding(withToast) {
  store.set("mv_onboarded", true);
  showScreen("home");
  if (withToast) {
    $("toast-wrap").classList.remove("hidden");
    clearTimeout(state.toastTimer);
    state.toastTimer = setTimeout(() => $("toast-wrap").classList.add("hidden"), 3200);
  }
}

function rememberLocally(id, type, text) {
  state.recent.unshift({ id: String(id), type, text, when: Date.now() });
  state.recent = state.recent.slice(0, 20);
  store.set("mv_recent", state.recent);
}

// -- home -------------------------------------------------------------------

function renderHome() {
  renderNav();
  renderTab();
  renderFooter();
}

function renderNav() {
  const items = [
    { label: "Memories", key: "memories" },
    { label: "Boundaries", key: "boundaries" },
    { label: "Agents", key: "agents" },
    { label: "Settings", key: "settings" },
  ];
  $("nav").innerHTML = items.map((n) =>
    `<span class="${state.tab === n.key ? "on" : ""}" data-k="${n.key}">${n.label}</span>`).join("");
  [...$("nav").children].forEach((el) => {
    el.addEventListener("click", () => { state.tab = el.dataset.k; renderTab(); });
  });
}

function renderTab() {
  renderNav2();
  for (const t of ["memories", "boundaries", "agents", "settings"]) {
    $(`tab-${t}`).classList.toggle("hidden", t !== state.tab);
  }
  if (state.tab === "memories") renderMemList();
  if (state.tab === "boundaries") renderBoundaries();
  if (state.tab === "agents") renderAgents();
  if (state.tab === "settings") renderSettings();
}

// re-style nav highlights without rebuilding listeners
function renderNav2() {
  [...$("nav").children].forEach((el) => el.classList.toggle("on", el.dataset.k === state.tab));
}

// -- memories tab --

async function renderMemList() {
  const q = state.query.trim();
  const seq = ++state.searchSeq;
  if (!q) {
    $("mem-list-title").textContent = "Recently remembered";
    renderMemRows(state.recent.map((m) => ({
      id: m.id, memory_type: m.type, content: m.text, when: relTime(m.when),
    })), "Nothing here yet — keep a memory below, or connect an agent and let it remember for you.");
    return;
  }
  $("mem-list-title").textContent = "Recalling…";
  try {
    const results = await invoke("search_memories", { query: q, limit: 8 });
    if (seq !== state.searchSeq) return; // stale response — a newer query is in flight
    $("mem-list-title").textContent = "Recalled";
    renderMemRows(results.map((r) => ({
      id: r.id, memory_type: r.memory_type, content: r.content, when: relTime(r.created_at),
    })), "Nothing recalled for that — yet.");
  } catch (err) {
    if (seq !== state.searchSeq) return;
    $("mem-list-title").textContent = "Recall failed";
    $("mem-rows").innerHTML = `<div class="empty-note">${esc(String(err))}</div>`;
  }
}

function renderMemRows(rows, emptyText) {
  if (!rows.length) {
    $("mem-rows").innerHTML = `<div class="empty-note">${esc(emptyText)}</div>`;
    return;
  }
  $("mem-rows").innerHTML = rows.map((m) => `
    <div class="mem-row" data-id="${esc(m.id)}">
      <span class="ty">${esc(TYPE_ROW_LABEL[m.memory_type] || m.memory_type)}</span>
      <span class="tx">${esc(m.content)}</span>
      <span class="wh">${esc(m.when)}</span>
      <button class="forget" title="Delete this memory">forget</button>
    </div>`).join("");
  [...$("mem-rows").querySelectorAll(".forget")].forEach((btn) => {
    btn.addEventListener("click", async () => {
      const row = btn.closest(".mem-row");
      const id = row.dataset.id;
      if (!confirm("Forget this memory? This cannot be undone.")) return;
      try {
        await invoke("delete_memory", { id });
        state.recent = state.recent.filter((m) => m.id !== id);
        store.set("mv_recent", state.recent);
        row.remove();
      } catch (err) {
        alert(`Couldn't forget it: ${err}`);
      }
    });
  });
}

function toggleInlineAdd(show) {
  state.showInlineAdd = show ?? !state.showInlineAdd;
  $("inline-add").classList.toggle("hidden", !state.showInlineAdd);
  if (state.showInlineAdd) {
    renderTypeChips("add-chips", state.addType, (t) => { state.addType = t; toggleInlineAdd(true); });
    $("add-text").focus();
  }
}

async function saveInlineMemory() {
  const text = $("add-text").value.trim();
  if (!text) return;
  $("add-err").textContent = "";
  try {
    const id = await invoke("add_memory", {
      content: text,
      memoryType: state.addType,
      boundary: "default",
    });
    rememberLocally(id, state.addType, text);
    $("add-text").value = "";
    toggleInlineAdd(false);
    state.query = "";
    $("search-input").value = "";
    renderMemList();
  } catch (err) {
    $("add-err").textContent = `Couldn't keep that memory: ${err}`;
  }
}

// -- boundaries tab --

function renderBoundaries() {
  // Slice 1 honesty: the engine enforces boundaries (mandatory access
  // control), but the UI has no list/create commands yet — the one
  // boundary guaranteed to exist is `default`, which this UI writes to.
  $("boundary-rows").innerHTML = `
    <div class="b-row">
      <span class="nm">default</span>
      <span class="ds">Everything this app remembers — agents read it once you grant access</span>
      <span class="mt">active</span>
    </div>`;
}

// -- agents tab --

function renderAgents() {
  if (!state.agents.length) {
    $("agent-rows").innerHTML = "";
    $("no-agents").classList.remove("hidden");
  } else {
    $("no-agents").classList.add("hidden");
    $("agent-rows").innerHTML = state.agents.map((a, i) => `
      <div class="a-row">
        <span class="st"></span>
        <span class="nm">${esc(a.name)}</span>
        <span class="tr">${esc(a.transport)}</span>
        <span class="ac">configured ${esc(relTime(a.when))}</span>
        <button class="revoke" data-i="${i}">remove</button>
      </div>`).join("");
    [...$("agent-rows").querySelectorAll(".revoke")].forEach((btn) => {
      btn.addEventListener("click", () => {
        state.agents.splice(Number(btn.dataset.i), 1);
        store.set("mv_agents", state.agents);
        renderAgents();
        renderFooter();
      });
    });
  }
  $("connect-panel-label").textContent = state.showConnectPanel
    ? "hide connection details" : "+ Connect an agent";
  $("agents-panel").classList.toggle("hidden", !state.showConnectPanel);
  $("agents-snippet").textContent = SNIPPET_JSON;
}

// -- settings tab --

async function renderSettings() {
  let vaultLocation = "per-user app data folder";
  try {
    if (window.__TAURI__.path && window.__TAURI__.path.appDataDir) {
      vaultLocation = await window.__TAURI__.path.appDataDir();
    }
  } catch { /* keep fallback label */ }
  const rows = [
    { label: "Encryption", value: "AES-256 · key in Windows Credential Manager", good: true },
    { label: "Vault location", value: vaultLocation },
    { label: "Recall engine", value: "on-device · works offline", good: true },
    { label: "Audit log", value: "recorded locally · UI + agent operations", good: true },
    { label: "Version", value: "memory-vault 0.1.0 · V0.2 beta" },
  ];
  $("settings-rows").innerHTML = rows.map((r) => `
    <div class="s-row">
      <span class="lbl">${esc(r.label)}</span>
      <span class="val${r.good ? " good" : ""}">${esc(r.value)}</span>
    </div>`).join("");
}

function replayWelcome() {
  state.tab = "memories";
  showScreen("welcome");
}

// -- footer --

function renderFooter() {
  const n = state.agents.length;
  $("footer-status").textContent = "Encrypted on this device · " +
    (n > 0 ? `${n} agent${n > 1 ? "s" : ""} configured` : "no agents configured yet");
}

// ---------------------------------------------------------------- wiring

function init() {
  // welcome
  $("begin-btn").addEventListener("click", () => {
    if (state.checksDone >= CHECK_DEFS.length) showScreen("connect");
  });

  // connect
  renderAgentCards();
  $("agent-grid").addEventListener("click", (e) => {
    const card = e.target.closest(".agent-card");
    if (!card) return;
    state.agentPicked = Number(card.dataset.i);
    renderAgentCards();
    $("snippet-wrap").scrollIntoView({ behavior: "smooth", block: "end" });
  });
  $("copy-snippet").addEventListener("click", () => {
    if (state.agentPicked !== null) copyText(AGENTS[state.agentPicked].snippet, $("copy-snippet"));
  });
  $("connect-cta").addEventListener("click", connectContinue);
  $("connect-skip").addEventListener("click", () => showScreen("memory"));

  // first memory
  renderMemoryScreen();
  $("mem-text").addEventListener("input", renderMemoryScreen);
  $("mem-save").addEventListener("click", saveFirstMemory);
  $("mem-skip").addEventListener("click", () => finishOnboarding(false));

  // home — search
  let debounce = null;
  $("search-input").addEventListener("input", () => {
    state.query = $("search-input").value;
    clearTimeout(debounce);
    debounce = setTimeout(renderMemList, 350);
  });
  $("search-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      clearTimeout(debounce);
      state.query = $("search-input").value;
      renderMemList();
    }
  });
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      if (state.screen === "home") {
        state.tab = "memories";
        renderTab();
        $("search-input").focus();
      }
    }
  });

  // home — inline add
  $("add-toggle").addEventListener("click", () => toggleInlineAdd());
  $("add-save").addEventListener("click", saveInlineMemory);
  $("add-cancel").addEventListener("click", () => toggleInlineAdd(false));

  // home — agents
  $("connect-panel-label").addEventListener("click", () => {
    state.showConnectPanel = !state.showConnectPanel;
    renderAgents();
  });
  $("copy-agents-snippet").addEventListener("click", () =>
    copyText(SNIPPET_JSON, $("copy-agents-snippet")));

  // home — settings
  $("replay-welcome").addEventListener("click", replayWelcome);

  showScreen(state.screen);
}

document.addEventListener("DOMContentLoaded", init);
