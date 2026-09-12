//! One modal at a time, and the two ways out of it. Everything the pages open
//! -- a task, an agent's terminal, the create form -- is a `scrim` over the
//! page, so closing is one rule in one place.

import { $, state } from "./core.js";
import { writeHash } from "./scopes.js";
import { stopTerminal } from "./terminal.js";

export function scrim(inner) {
  const el = document.createElement("div");
  el.className = "scrim";
  el.innerHTML = `<div class="modal">${inner}</div>`;
  el.onclick = (e) => { if (e.target === el) closeModal(); };
  document.body.appendChild(el);
  return el;
}

/// Tear the modal down and leave the URL alone. For the places that put another
/// modal up in the same breath -- opening a task over a task, the edit form over
/// the task it edits -- where a URL written in between would name a page nobody
/// was ever on and cost a Back press to get past.
export function dropModal() {
  for (const el of document.querySelectorAll(".scrim")) el.remove();
  state.open = null; state.runs = []; state.run = null;
  stopTerminal();
  state.term = null;
}

/// Closing is a navigation: an open task is in the URL, so shutting it has to
/// come back out of the URL. Harmless over a modal that was never routed -- the
/// hash comes out the same and nothing is written.
export function closeModal() {
  dropModal();
  writeHash();
}

document.addEventListener("keydown", (e) => {
  // Escape closes the modal -- unless the terminal input has focus, where it is
  // a key the agent is waiting for.
  if (e.key !== "Escape") return;
  if (document.activeElement && document.activeElement.id === "t-term") return;
  closeModal();
});

/// The terminal plus the row that types into it. Same markup for a task run and
