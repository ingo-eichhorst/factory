//! The always-on terminal: the markup, the poll that keeps it current, and the
//! row that types back into the session. The same block serves a task run and a
//! standing agent -- only the endpoint differs.

import { $, esc, api, state, TERMINAL } from "./core.js";

export function terminalBlock(label) {
  return `
    <label>${label} <span class="live-tag" id="t-live" hidden>live</span></label>
    <pre class="term" id="t-term">connecting…</pre>
    <div class="terminput">
      <input id="t-input" placeholder="type here, Enter sends" autocomplete="off">
      <span class="keys">
        <button class="btn" data-key="enter">⏎</button>
        <button class="btn" data-key="esc">esc</button>
        <button class="btn" data-key="up">↑</button>
        <button class="btn" data-key="down">↓</button>
        <button class="btn" data-key="ctrl-c">^C</button>
      </span>
    </div>
    <div class="err" id="t-err"></div>`;
}

export function wireTerminal() {
  const input = $("t-input");
  if (!input) return;
  input.onkeydown = (e) => {
    if (e.key === "Enter") { e.preventDefault(); sendInput(input.value, ["enter"]); input.value = ""; }
  };
  for (const b of document.querySelectorAll("[data-key]")) {
    b.onclick = () => sendInput("", [b.dataset.key]);
  }
}

// transcript afterwards. The daemon answers with empty text rather than an
// error in the seconds before a session is up, so this never flashes red.

export function stopTerminal() {
  if (state.poll) { clearInterval(state.poll); state.poll = null; }
}

export function setTerminal(kind, id, live) {
  stopTerminal();
  state.term = id ? { kind, id } : null;
  if ($("t-live")) $("t-live").hidden = !live;
  if ($("t-input")) $("t-input").disabled = !live;
  if (!state.term) {
    if ($("t-term")) { $("t-term").textContent = "No run yet. Press Run to start one."; $("t-term").classList.add("idle"); }
    return;
  }
  fetchTerminal();
  if (live) state.poll = setInterval(fetchTerminal, 1500);
}

export async function fetchTerminal() {
  const el = $("t-term");
  const target = state.term;
  if (!el || !target) return;
  try {
    const data = target.kind === "run"
      ? await api(`/api/runs/${target.id}/output?lines=400`)
      : await api("/api/agents/output", { method: "POST", body: JSON.stringify({ id: target.id, lines: 400 }) });
    if (!state.term || state.term.id !== target.id || !$("t-term")) return;  // the view moved on
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    if (data.text.trim()) {
      el.classList.remove("idle");
      el.textContent = data.text;
    } else {
      el.classList.add("idle");
      el.textContent = "Waiting for the session to come up…";
    }
    if (atBottom) el.scrollTop = el.scrollHeight;
  } catch (e) {
    el.classList.add("idle");
    el.textContent = e.message;
  }
}

export async function sendInput(text, keys) {
  const target = state.term;
  if (!target) return;
  if ($("t-err")) $("t-err").textContent = "";
  try {
    const body = JSON.stringify(target.kind === "run"
      ? { text, keys }
      : { id: target.id, text, keys });
    await api(target.kind === "run" ? `/api/runs/${target.id}/input` : "/api/agents/input",
              { method: "POST", body });
    setTimeout(fetchTerminal, 250);
  } catch (e) {
    if ($("t-err")) $("t-err").textContent = e.message;
  }
}
