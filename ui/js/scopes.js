//! The scope rail: the tree beside both views, and the one predicate that says
//! whether a thing belongs to what is selected.
//!
//! The tree is derived, never declared. A `ScopeView` carries a `path` and
//! nothing about nesting, so the shape is whatever the paths say -- and a path
//! segment can turn up with no scope registered on it (`projects`, on the
//! instance that builds this repo). Those segments are drawn, or everything
//! under them would hang off the root as if it were flat; they are not
//! selectable, because nothing in the daemon can be asked about a segment that
//! is not a scope.
//!
//! This file filters nothing and knows about no view. It owns the selection,
//! the rail and the URL; every view asks `inScope` and draws itself.
//!
//! `initRail`'s callback is handed the route it is reacting to -- `{scope,
//! tab}` -- because back and forward move the tab as well as the selection, and
//! the tabs belong to the page. A callback that only cares about the selection
//! can take no arguments and ignore it.

import { $, esc, state } from "./core.js";

/// The hash segment that stands for no selection at all.
const ALL = "all";
const TABS = ["tasks", "agents"];

let onSelect = () => {};
let wired = false;
/// The markup the rail is currently showing, so an unchanged tree is left
/// alone. See `render`.
let drawn = "";
/// The hash this file last put there, so it can tell its own writing apart from
/// someone pressing Back.
let written = "";

// ----------------------------------------------------------------- the tree

/// A path as the tree reads it. `ScopeView.path` is the absolute working
/// directory -- `scope_path` joins the instance root onto whatever the config
/// wrote -- so every path begins with the root's own segments and the scope
/// registered on the root is a prefix of all the others.
///
/// Those leading segments are where the instance happens to live, not anything
/// the config declared, so they are cut off: left in, the trie grows one
/// unselectable grouping node per segment of the root -- `USERS` above
/// `FACTORY` above the whole tree -- and pushes every real scope that much
/// further right. What is left is the path as `.factory/config.yaml` wrote it,
/// which is the shape the rail is supposed to show. A scope configured with an
/// absolute path outside the root matches nothing here and keeps its own
/// segments, which is the honest answer: it really does sit somewhere else.
function segments(path) {
  let p = String(path ?? "");
  const root = String(state.root ?? "").replace(/\/+$/, "");
  if (root && (p === root || p.startsWith(`${root}/`))) p = p.slice(root.length);
  return p.split("/").filter(s => s !== "" && s !== ".");
}

/// Every path threaded onto one trie, so the nesting falls out of the segments
/// instead of being computed by comparing paths to each other -- a trie node is
/// a whole segment, so `projects/factory` can never swallow `projects/factory-x`.
/// A node with no `scope` is a segment nobody registered.
function buildTree() {
  const root = { label: null, scope: null, children: new Map() };
  for (const scope of state.scopes) {
    let node = root;
    for (const seg of segments(scope.path)) {
      if (!node.children.has(seg)) {
        node.children.set(seg, { label: seg, scope: null, children: new Map() });
      }
      node = node.children.get(seg);
    }
    // Two scopes on one path is a mistake in the config rather than a shape to
    // draw twice. The first one registered keeps the node; the second is still
    // reachable through its parent's selection, because `inScope` reads paths.
    if (!node.scope) node.scope = scope;
  }
  return root;
}

function branch(node, depth) {
  if (!node.children.size) return "";
  // Registration order, not alphabetical: the config is a list someone wrote in
  // an order, and the roster already shows the scopes in it.
  return `<ul>${[...node.children.values()].map(c => row(c, depth)).join("")}</ul>`;
}

function row(node, depth) {
  const on = node.scope && node.scope.name === state.scope ? ` aria-current="true"` : "";
  // A deep name does not fit the column and is cut short there, so the tooltip
  // carries the whole of it as well as the path it sits on.
  const head = node.scope
    ? `<button class="rail-row" style="--d:${depth}" data-scope="${esc(node.scope.name)}" title="${esc(node.scope.name)} · ${esc(node.scope.path)}"${on}>${esc(node.scope.name)}</button>`
    : `<span class="rail-row group" style="--d:${depth}">${esc(node.label)}</span>`;
  return `<li>${head}${branch(node, depth + 1)}</li>`;
}

function render() {
  const rail = $("rail");
  if (!rail) return;

  const root = buildTree();
  // No scopes is a real state -- a fresh install has none -- and an empty tree
  // drawn around it looks like a page that failed to load. A scope on the
  // instance root owns the trie's own root, so it becomes the one row everything
  // else hangs under; with none, whatever is shallowest is a root, and there can
  // be several.
  const html = !state.scopes.length
    ? `<p class="rail-empty">This instance declares no scopes.</p>`
    : `
    <button class="rail-row rail-all" id="rail-all"${state.scope === null ? ` aria-current="true"` : ""}>All scopes</button>
    ${root.scope ? `<ul>${row(root, 0)}</ul>` : branch(root, 0)}`;

  // `loadAgents` rebuilds the rail on every agent event, and replacing the
  // markup takes the focus with it -- a keyboard user would lose their place
  // mid-interaction, several times a minute. The selection is written into the
  // markup, so an identical string is an unchanged rail. Compared against what
  // was last written and never against `innerHTML`, which comes back out of the
  // browser normalised and would never match.
  if (html === drawn) return;
  drawn = html;
  rail.innerHTML = html;
  if (!state.scopes.length) return;

  $("rail-all").onclick = () => select(null);
  for (const el of rail.querySelectorAll("[data-scope]")) {
    el.onclick = () => select(el.dataset.scope);
  }
}

// ------------------------------------------------------------ the selection

/// The name, but only if the tree still has it: a link outlives the scope it
/// names, and a refreshed `/api/agents` can drop one. A grouping node never
/// gets here -- it carries no name to click.
function known(name) {
  if (!name) return null;
  return state.scopes.some(s => s.name === name) ? name : null;
}

function select(name) {
  if (name === state.scope) return;
  state.scope = name;
  render();
  writeHash(state.tab);
  onSelect({ scope: state.scope, tab: state.tab });
}

export function initRail(onChange) {
  if (onChange) onSelect = onChange;
  const first = !wired;
  const route = readHash();
  const before = state.scope;

  // On the first call the hash is the boot selection; on later ones -- after
  // `/api/agents` came back again -- the hash has long been normalised and what
  // matters is whether the tree still holds what is selected.
  state.scope = known(first && route ? route.scope : state.scope);
  render();

  if (first) {
    window.addEventListener("hashchange", onHashChange);
    wired = true;
  }
  const tab = route && TABS.includes(route.tab) ? route.tab : state.tab;
  writeHash(tab, true);

  // A link straight to `#factory/agents` has to reach the page as a change even
  // though nothing was selected before it; after boot, only a real change is.
  if (state.scope !== before || (first && route)) onSelect({ scope: state.scope, tab });
}

/// True when `scopeName` is inside the selection, inclusive of the selection
/// itself and of everything nested under it.
export function inScope(scopeName) {
  if (state.scope === null) return true;
  if (scopeName === state.scope) return true;
  // Fail open while the tree is unknown. The websocket snapshot renders tasks
  // before `/api/agents` has answered, and a selection that cannot be resolved
  // to a path would hide every row on that tick -- a blank page, once, on every
  // reconnect. The same goes for a selection the tree no longer has: `initRail`
  // clears it a moment later, and until then showing everything is the honest
  // answer.
  const selected = state.scopes.find(s => s.name === state.scope);
  if (!selected) return true;
  const here = state.scopes.find(s => s.name === scopeName);
  if (!here) return false;
  return under(segments(here.path), segments(selected.path));
}

/// Containment on segment boundaries, so `projects/factory` holds
/// `projects/factory/reviews/skeptic` and not `projects/factory-x`. A scope on
/// the instance root is a prefix of every other path, so everything is under it.
function under(path, parent) {
  return parent.every((seg, i) => path[i] === seg);
}

export function scopeLabel() {
  return state.scope === null ? "All scopes" : state.scope;
}

// -------------------------------------------------------------------- the URL

/// `#<scope>/<tab>`, so a link names both halves of what is on screen. A click
/// is a navigation and pushes; `replace` is for the writes that only say what is
/// already on screen -- boot, and correcting a hash that named a scope the tree
/// no longer has -- because an entry pushed there costs a Back press to get past
/// and is not somewhere the user has been.
export function writeHash(tab, replace) {
  // `all` is the keyword for no selection, so a scope actually called `all` has
  // to be written as something that decodes back to its name without reading as
  // the keyword. Every other name survives encodeURIComponent unchanged.
  const head = state.scope === null ? ALL
    : state.scope === ALL ? "%61ll" : encodeURIComponent(state.scope);
  const next = `#${head}/${tab}`;
  written = next;
  // Assigning the hash it already carries would push nothing anyway; assigning
  // a different one fires `hashchange`, and this runs on every tab switch.
  if (location.hash === next) return;
  if (replace) history.replaceState(null, "", next);
  else location.hash = next;
}

/// `{scope, tab}` for a hash that names a route, `null` for anything else.
/// Deliberately syntax only: boot reads the hash before `/api/agents` has
/// answered, so there is no tree yet to check the name against. `initRail` does
/// that part.
export function readHash() {
  const raw = location.hash.replace(/^#/, "");
  const cut = raw.lastIndexOf("/");
  if (cut < 0) return null;
  const tab = raw.slice(cut + 1);
  if (!TABS.includes(tab)) return null;
  const head = raw.slice(0, cut);
  if (head === ALL) return { scope: null, tab };
  // A hash can be typed by hand, and a broken escape in one makes
  // decodeURIComponent throw. A route nobody can read is no route.
  try {
    return { scope: decodeURIComponent(head), tab };
  } catch {
    return null;
  }
}

function onHashChange() {
  // Our own writes land here too, one event behind. Nothing has moved then, and
  // reacting would draw every selection twice.
  if (location.hash === written) return;
  written = location.hash;
  const route = readHash();
  if (!route) return;
  // A hash naming a scope that is gone lands on "All scopes", and the URL is
  // corrected where it stands rather than pushed: an entry written on top of the
  // one the user just went back to is an entry Back can never get past.
  state.scope = known(route.scope);
  writeHash(route.tab, true);
  render();
  // Back and forward move the tab as well as the selection, and the tabs are
  // the page's. Whoever wired the rail up gets told what the URL now says.
  onSelect({ scope: state.scope, tab: route.tab });
}
