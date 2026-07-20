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

// -- confirmation ------------------------------------------------------------

// Ask the user to confirm a destructive action; resolves true only if they
// actively choose to proceed.
//
// Why this exists: the webview renders the native `window.confirm` dialog as
// an OK-only message box — there is no Cancel, so the user CANNOT decline and
// the action goes ahead whatever they click. Found in live verification
// 2026-07-20 on both `forget` (permanent delete) and `revoke`. A gate that
// cannot say no is not a gate. `tests/frontend_contract.rs` now fails the
// build if a native gating dialog is reintroduced anywhere in this file.
//
// Cancel is the visually dominant button and takes initial focus, so Enter
// and Esc both mean "don't". Text is set via textContent, never innerHTML —
// the agent name interpolated into the revoke prompt is untrusted input.
function confirmAction({ title, body, confirmLabel }) {
  return new Promise((resolve) => {
    const overlay = $("confirm-overlay");
    const go = $("confirm-go");
    const cancel = $("confirm-cancel");

    $("confirm-title").textContent = title;
    $("confirm-body").textContent = body;
    go.textContent = confirmLabel;
    overlay.classList.remove("hidden");
    cancel.focus();

    function settle(answer) {
      overlay.classList.add("hidden");
      go.removeEventListener("click", onGo);
      cancel.removeEventListener("click", onCancel);
      overlay.removeEventListener("mousedown", onBackdrop);
      document.removeEventListener("keydown", onKey);
      resolve(answer);
    }
    function onGo() { settle(true); }
    function onCancel() { settle(false); }
    // Backdrop click dismisses — but only when the press STARTED on the
    // backdrop, so a drag that ends outside the box doesn't cancel.
    function onBackdrop(e) { if (e.target === overlay) settle(false); }
    function onKey(e) {
      if (e.key === "Escape") { e.preventDefault(); settle(false); }
    }

    go.addEventListener("click", onGo);
    cancel.addEventListener("click", onCancel);
    overlay.addEventListener("mousedown", onBackdrop);
    document.addEventListener("keydown", onKey);
  });
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
  // `agents` is the local record of which agent the user set up a config
  // snippet for during onboarding — it drives the connect UI only. The
  // authoritative list of agents holding vault access is `grantedAgents`,
  // read from the backend registry (slice 2).
  agents: store.get("mv_agents", []),
  grantedAgents: [],                         // from list_agents (authoritative)
  boundaries: [],                            // from list_boundaries
  showConnectPanel: false,
  showInlineAdd: false,
  showBoundaryAdd: false,
  toastTimer: null,
  searchSeq: 0,
  recentSeq: 0,
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
    await invoke("add_memory", {
      content: text,
      memoryType: state.memType,
      boundary: "default",
    });
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

// Slice 2 removed the browser-local `mv_recent` cache: the home list now
// reads `list_recent_memories` from the vault itself, so memories an AGENT
// wrote show up alongside the ones added here. Clear the stale cache once so
// upgrading installs don't leave dead data in localStorage.
try { localStorage.removeItem("mv_recent"); } catch { /* non-fatal */ }

// -- home -------------------------------------------------------------------

function renderHome() {
  renderNav();
  renderTab();
  renderFooter();
  // The footer reports how many agents hold access, so it needs the registry
  // even when the user never opens the Agents tab. Fire-and-forget: a failed
  // read leaves the footer at its last known value rather than blocking home.
  refreshGrantedAgents();
}

async function refreshGrantedAgents() {
  try {
    state.grantedAgents = await invoke("list_agents");
    renderFooter();
  } catch { /* footer keeps its last known value */ }
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
    const rseq = ++state.recentSeq;
    try {
      const recent = await invoke("list_recent_memories", { limit: 20 });
      if (rseq !== state.recentSeq || state.query.trim()) return; // superseded
      renderMemRows(recent.map((m) => ({
        id: m.id, memory_type: m.memory_type, content: m.content, when: relTime(m.created_at),
      })), "Nothing here yet — keep a memory below, or connect an agent and let it remember for you.");
    } catch (err) {
      if (rseq !== state.recentSeq) return;
      $("mem-rows").innerHTML = `<div class="empty-note">Couldn't load your memories: ${esc(String(err))}</div>`;
    }
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
      const ok = await confirmAction({
        title: "Forget this memory?",
        body: "It will be permanently deleted from your vault straight away. "
            + "There is no undo and no recovery period.",
        confirmLabel: "Forget it",
      });
      if (!ok) return;
      try {
        await invoke("delete_memory", { id });
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
    await invoke("add_memory", {
      content: text,
      memoryType: state.addType,
      boundary: "default",
    });
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

async function renderBoundaries() {
  $("boundary-rows").innerHTML = `<div class="empty-note">Loading…</div>`;
  try {
    const rows = await invoke("list_boundaries");
    state.boundaries = rows;
    $("boundary-rows").innerHTML = rows.map((b) => {
      const n = Number(b.memory_count) || 0;
      const count = n === 1 ? "1 memory" : `${n} memories`;
      const desc = b.description
        || (b.name === "default" ? "Everything this app remembers by default" : "");
      return `
      <div class="b-row">
        <span class="nm">${esc(b.name)}</span>
        <span class="ds">${esc(desc)}</span>
        <span class="mt">${esc(count)}</span>
      </div>`;
    }).join("");
  } catch (err) {
    $("boundary-rows").innerHTML = `<div class="empty-note">Couldn't load boundaries: ${esc(String(err))}</div>`;
  }
  $("boundary-add-label").textContent = state.showBoundaryAdd ? "cancel" : "+ New boundary";
  $("boundary-add").classList.toggle("hidden", !state.showBoundaryAdd);
}

function toggleBoundaryAdd(show) {
  state.showBoundaryAdd = show ?? !state.showBoundaryAdd;
  $("boundary-err").textContent = "";
  $("boundary-add").classList.toggle("hidden", !state.showBoundaryAdd);
  $("boundary-add-label").textContent = state.showBoundaryAdd ? "cancel" : "+ New boundary";
  if (state.showBoundaryAdd) $("boundary-name").focus();
}

async function saveBoundary() {
  const name = $("boundary-name").value.trim();
  const description = $("boundary-desc").value.trim();
  if (!name) return;
  $("boundary-err").textContent = "";
  try {
    const created = await invoke("create_boundary", {
      name,
      description: description || null,
    });
    if (!created) {
      $("boundary-err").textContent = "You already have a boundary with that name.";
      return;
    }
    $("boundary-name").value = "";
    $("boundary-desc").value = "";
    toggleBoundaryAdd(false);
    renderBoundaries();
  } catch (err) {
    // The backend rejects anything outside letters, digits, - and _ ; say so
    // in plain language rather than surfacing the validation error verbatim.
    $("boundary-err").textContent =
      `Couldn't create that boundary: ${String(err).replace(/^invalid boundary name: /, "")}`;
  }
}

// -- agents tab --

async function renderAgents() {
  // Slice 2: the registry of agents that actually hold vault access, not the
  // browser-local list of cards the user clicked during onboarding. An agent
  // appears here once it has been granted a connection token.
  let agents = [];
  try {
    agents = await invoke("list_agents");
    state.grantedAgents = agents;
    renderFooter();
  } catch (err) {
    $("agent-rows").innerHTML = `<div class="empty-note">Couldn't load agents: ${esc(String(err))}</div>`;
    $("no-agents").classList.add("hidden");
    $("connect-panel-label").textContent = state.showConnectPanel
      ? "hide connection details" : "+ Connect an agent";
    $("agents-panel").classList.toggle("hidden", !state.showConnectPanel);
    $("agents-snippet").textContent = SNIPPET_JSON;
    return;
  }

  if (!agents.length) {
    $("agent-rows").innerHTML = "";
    $("no-agents").classList.remove("hidden");
  } else {
    $("no-agents").classList.add("hidden");
    $("agent-rows").innerHTML = agents.map((a) => {
      const scope = a.boundaries.length
        ? `can read: ${a.boundaries.join(", ")}`
        : "no boundaries granted";
      const when = a.active
        ? `connected ${relTime(a.created_at)}`
        : `revoked ${relTime(a.revoked_at)}`;
      return `
      <div class="a-row${a.active ? "" : " revoked"}" data-name="${esc(a.name)}">
        <span class="st"></span>
        <span class="nm">${esc(a.name)}</span>
        <span class="tr">${esc(scope)}</span>
        <span class="ac">${esc(when)}</span>
        ${a.active ? `<button class="revoke">revoke</button>` : `<span class="ac">—</span>`}
      </div>`;
    }).join("");
    [...$("agent-rows").querySelectorAll(".revoke")].forEach((btn) => {
      btn.addEventListener("click", async () => {
        const row = btn.closest(".a-row");
        const name = row.dataset.name;
        const ok = await confirmAction({
          title: `Revoke ${name}'s access?`,
          body: `${name} won't be able to read or write memories until you `
              + `connect it again. It stays listed here, and everything it `
              + `did before stays in the audit log.`,
          confirmLabel: "Revoke access",
        });
        if (!ok) return;
        try {
          await invoke("revoke_agent", { agentName: name });
          renderAgents();
        } catch (err) {
          alert(`Couldn't revoke access: ${err}`);
        }
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
  $("settings-rows").innerHTML = `<div class="empty-note">Loading…</div>`;
  let info = null;
  try {
    info = await invoke("get_settings_info");
  } catch (err) {
    $("settings-rows").innerHTML = `<div class="empty-note">Couldn't read vault details: ${esc(String(err))}</div>`;
    return;
  }

  const n = Number(info.memory_count) || 0;
  const b = Number(info.boundary_count) || 0;
  const rows = [
    { label: "Encryption", value: "AES-256 · key in Windows Credential Manager", good: true },
    { label: "Vault location", value: info.data_dir },
    {
      label: "Stored",
      value: `${n === 1 ? "1 memory" : `${n} memories`} across ${b === 1 ? "1 boundary" : `${b} boundaries`}`,
    },
    { label: "Recall engine", value: "on-device · works offline", good: true },
    // Honest UI: report what the chain check actually returned. A broken
    // chain means something modified the history outside the app, and the
    // user needs to know rather than see a reassuring constant.
    info.audit_chain_verified
      ? { label: "Audit log", value: "recorded locally · history verified", good: true }
      : { label: "Audit log", value: "history could not be verified — the record may have been altered", good: false },
    { label: "Version", value: `memory-vault ${info.version} · V0.2 beta` },
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
  // Counts agents that actually hold vault access (the backend registry),
  // not agents the user merely copied a config snippet for.
  const n = state.grantedAgents.filter((a) => a.active).length;
  $("footer-status").textContent = "Encrypted on this device · " +
    (n > 0 ? `${n} agent${n > 1 ? "s" : ""} connected` : "no agents connected yet");
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

  // home — boundaries
  $("boundary-add-label").addEventListener("click", () => toggleBoundaryAdd());
  $("boundary-save").addEventListener("click", saveBoundary);
  $("boundary-cancel").addEventListener("click", () => toggleBoundaryAdd(false));
  $("boundary-name").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); saveBoundary(); }
  });
  $("boundary-desc").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); saveBoundary(); }
  });

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
