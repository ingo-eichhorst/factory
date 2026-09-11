//! One modal at a time, and the two ways out of it. Everything the pages open
//! -- a task, an agent's terminal, the create form -- is a `scrim` over the
//! page, so closing is one rule in one place.

import { $, state } from "./core.js";
import { stopTerminal } from "./terminal.js";

export function scrim(inner) {
  const el = document.createElement("div");
  el.className = "scrim";
  el.innerHTML = `<div class="modal">${inner}</div>`;
  el.onclick = (e) => { if (e.target === el) closeModal(); };
  document.body.appendChild(el);
  return el;
}

export function closeModal() {
  for (const el of document.querySelectorAll(".scrim")) el.remove();
  state.open = null; state.runs = []; state.run = null;
  stopTerminal();
  state.term = null;
}

document.addEventListener("keydown", (e) => {
  // Escape closes the modal -- unless the terminal input has focus, where it is
  // a key the agent is waiting for.
  if (e.key !== "Escape") return;
  if (document.activeElement && document.activeElement.id === "t-input") return;
  closeModal();
});

/// The terminal plus the row that types into it. Same markup for a task run and
