//! The terminal: a real one, not a transcript.
//!
//! The daemon pushes whole frames over a socket -- the grid herdr has already
//! rendered, text plus colour and nothing else -- and every keystroke goes back
//! as the bytes a terminal would send. So arrows, Tab, Ctrl-C and an agent's own
//! escape hatch all work, without a table of key names in the middle deciding
//! what a keyboard is allowed to do.
//!
//! A run that has ended has no session left to mirror. Then this falls back to
//! the transcript the daemon kept, fetched once: nothing is changing any more.

import { $, esc, api, state } from "./core.js";

export function terminalBlock(label) {
  return `
    <label>${label}
      <span class="live-tag" id="t-live" hidden>live</span>
      <span class="term-hint" id="t-hint" hidden>click the screen and type</span>
    </label>
    <pre class="term" id="t-term" tabindex="0">connecting…</pre>
    <div class="err" id="t-err"></div>`;
}

/// What a terminal puts on the wire for a key. Anything printable is itself;
/// the rest is the sequence a VT-style terminal sends, which is what the agent
/// on the other end was written to read.
const KEYS = {
  Enter: "\r", Tab: "\t", Backspace: "\x7f", Escape: "\x1b", Delete: "\x1b[3~",
  ArrowUp: "\x1b[A", ArrowDown: "\x1b[B", ArrowRight: "\x1b[C", ArrowLeft: "\x1b[D",
  Home: "\x1b[H", End: "\x1b[F", PageUp: "\x1b[5~", PageDown: "\x1b[6~",
};

function bytesFor(e) {
  if (e.metaKey) return null;                        // leave cmd+C and friends to the browser
  if (e.ctrlKey) {
    // Ctrl-A is 0x01 and so on up to Ctrl-_: the arithmetic is the whole rule.
    const c = e.key.length === 1 ? e.key.toUpperCase().charCodeAt(0) : 0;
    if (c >= 64 && c < 96) return String.fromCharCode(c - 64);
    if (e.key === " ") return "\x00";
    return null;
  }
  if (KEYS[e.key]) return KEYS[e.key];
  if (e.key.length === 1) return e.key;
  return null;                                       // F-keys, modifiers, dead keys
}

export function wireTerminal() {
  const term = $("t-term");
  if (!term) return;
  term.onkeydown = (e) => {
    const bytes = bytesFor(e);
    if (bytes === null) return;
    e.preventDefault();
    send(bytes);
  };
  // The screen is the only way in now, so say plainly when it has the keys.
  term.onfocus = () => { term.classList.add("focused"); $("t-hint")?.classList.add("on"); };
  term.onblur = () => { term.classList.remove("focused"); $("t-hint")?.classList.remove("on"); };
}

export function stopTerminal() {
  if (state.poll) { clearInterval(state.poll); state.poll = null; }
  if (state.termSocket) {
    // Drop the handler first: a close we asked for must not look like one we
    // lost, or it reconnects to a session the page has already left.
    state.termSocket.onclose = null;
    state.termSocket.close();
    state.termSocket = null;
  }
}

export function setTerminal(kind, id, live) {
  stopTerminal();
  state.term = id ? { kind, id } : null;
  if ($("t-live")) $("t-live").hidden = !live;
  if ($("t-hint")) $("t-hint").hidden = !live;
  if (!state.term) {
    if ($("t-term")) { $("t-term").textContent = "No run yet. Press Run to start one."; $("t-term").classList.add("idle"); }
    return;
  }
  if (live) openTermSocket(); else fetchTerminal();
}

// ------------------------------------------------------------ the live frame

function openTermSocket() {
  const target = state.term;
  if (!target) return;
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(
    `${scheme}://${location.host}/ws/term?kind=${encodeURIComponent(target.kind)}&id=${encodeURIComponent(target.id)}`);
  state.termSocket = ws;
  ws.onmessage = (m) => {
    if (!state.term || state.term.id !== target.id) return;
    const el = $("t-term");
    if (!el) return;
    let msg; try { msg = JSON.parse(m.data); } catch { return; }
    if (msg.error) {
      // No session to mirror. Say what the daemon said rather than a blank box.
      el.classList.add("idle");
      el.textContent = msg.error;
      return;
    }
    el.classList.remove("idle");
    el.style.setProperty("--cols", msg.cols);
    el.innerHTML = renderFrame(msg.frame);
  };
  ws.onclose = () => {
    if (state.termSocket !== ws || !state.term) return;
    state.termSocket = null;
    // A daemon restart or a dropped connection should not leave a dead screen.
    setTimeout(() => { if (state.term && state.term.id === target.id) openTermSocket(); }, 1200);
  };
}

function send(bytes) {
  if (!bytes) return;
  if ($("t-err")) $("t-err").textContent = "";
  const ws = state.termSocket;
  if (ws && ws.readyState === WebSocket.OPEN) { ws.send(bytes); return; }
  if ($("t-err")) $("t-err").textContent = "no live session to type into";
}

// ------------------------------------------------------------- ANSI to HTML

const BASE = ["#2a2723", "#c14a3d", "#4a9c5c", "#b08a2e", "#4a7fb5", "#9c5fb0", "#3f9c9c", "#c9c4ba"];
const BRIGHT = ["#6d6862", "#e07a66", "#7cc088", "#d6b55c", "#7aa8d9", "#c08fd6", "#6fc7c7", "#f2efea"];

/// herdr hands back a rendered grid: the only escapes left in it are SGR, the
/// ones that set colour and weight. Every cursor move and every scroll has
/// already been applied, which is why this needs no terminal emulator -- it is
/// one span per colour run, and nothing more. Anything else that turns up is
/// dropped rather than printed as mojibake.
export function renderFrame(frame) {
  const sgr = /\x1b\[([0-9;:]*)m/g;
  const otherEscape = /\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-9;:?]*[a-zA-Z]|\x1b[^[\]]/g;
  const style = { fg: null, bg: null, bold: false, dim: false, italic: false, under: false, rev: false };
  let out = "", open = false, last = 0, m;

  const text = (raw) => { out += esc(raw.replace(otherEscape, "").replace(/\r/g, "")); };
  const restyle = () => {
    if (open) { out += "</span>"; open = false; }
    const s = css(style);
    if (s) { out += `<span style="${s}">`; open = true; }
  };

  while ((m = sgr.exec(frame)) !== null) {
    text(frame.slice(last, m.index));
    last = sgr.lastIndex;
    applySgr(m[1], style);
    restyle();
  }
  text(frame.slice(last));
  if (open) out += "</span>";
  return out;
}

function css(style) {
  let { fg, bg } = style;
  // Reverse video is how a rendered grid marks the cursor, so it has to survive
  // into the page or there is nothing to show where typing will land.
  if (style.rev) { const t = fg || "var(--term-bg)"; fg = bg || "var(--term-ink)"; bg = t; }
  const parts = [];
  if (fg) parts.push(`color:${fg}`);
  if (bg) parts.push(`background:${bg}`);
  if (style.bold) parts.push("font-weight:600");
  if (style.dim) parts.push("opacity:.65");
  if (style.italic) parts.push("font-style:italic");
  if (style.under) parts.push("text-decoration:underline");
  return parts.join(";");
}

function applySgr(params, style) {
  const codes = (params === "" ? "0" : params).split(";").map(n => parseInt(n, 10) || 0);
  for (let i = 0; i < codes.length; i++) {
    const c = codes[i];
    if (c === 0) { style.fg = style.bg = null; style.bold = style.dim = style.italic = style.under = style.rev = false; }
    else if (c === 1) style.bold = true;
    else if (c === 2) style.dim = true;
    else if (c === 3) style.italic = true;
    else if (c === 4) style.under = true;
    else if (c === 7) style.rev = true;
    else if (c === 22) { style.bold = false; style.dim = false; }
    else if (c === 23) style.italic = false;
    else if (c === 24) style.under = false;
    else if (c === 27) style.rev = false;
    else if (c === 39) style.fg = null;
    else if (c === 49) style.bg = null;
    else if (c >= 30 && c <= 37) style.fg = BASE[c - 30];
    else if (c >= 40 && c <= 47) style.bg = BASE[c - 40];
    else if (c >= 90 && c <= 97) style.fg = BRIGHT[c - 90];
    else if (c >= 100 && c <= 107) style.bg = BRIGHT[c - 100];
    else if (c === 38 || c === 48) {
      // 38;2;r;g;b is truecolour, 38;5;n the 256-colour cube.
      const target = c === 38 ? "fg" : "bg";
      if (codes[i + 1] === 2) { style[target] = `rgb(${codes[i + 2] | 0},${codes[i + 3] | 0},${codes[i + 4] | 0})`; i += 4; }
      else if (codes[i + 1] === 5) { style[target] = xterm256(codes[i + 2] | 0); i += 2; }
    }
  }
}

function xterm256(n) {
  if (n < 8) return BASE[n];
  if (n < 16) return BRIGHT[n - 8];
  if (n < 232) {
    const v = n - 16;
    const step = (x) => (x ? x * 40 + 55 : 0);
    return `rgb(${step(Math.floor(v / 36))},${step(Math.floor(v / 6) % 6)},${step(v % 6)})`;
  }
  const g = (n - 232) * 10 + 8;
  return `rgb(${g},${g},${g})`;
}

// ------------------------------------------------- the transcript, afterwards

/// What is left once a run has ended and its session is released.
export async function fetchTerminal() {
  const el = $("t-term");
  const target = state.term;
  if (!el || !target) return;
  try {
    const data = target.kind === "run"
      ? await api(`/api/runs/${target.id}/output?lines=400`)
      : await api("/api/agents/output", { method: "POST", body: JSON.stringify({ id: target.id, lines: 400 }) });
    if (!state.term || state.term.id !== target.id || !$("t-term")) return;  // the view moved on
    if (data.text.trim()) {
      el.classList.remove("idle");
      el.textContent = data.text;
    } else {
      el.classList.add("idle");
      el.textContent = "Waiting for the session to come up…";
    }
    el.scrollTop = el.scrollHeight;
  } catch (e) {
    el.classList.add("idle");
    el.textContent = e.message;
  }
}
