//! The scope rail: the tree beside both views, and the one predicate that says
//! whether a thing belongs to what is selected.
//!
//! The tree is derived, never declared. A `ScopeView` carries a `path` and
//! nothing about nesting, so the shape is whatever the paths say. Only a
//! directory with its own Factory scope config is represented; intervening
//! ordinary directories do not get rows of their own.
//!
//! This file filters nothing and knows about no view. It owns the selection,
//! the rail and the URL; every view asks `inScope` and draws itself.
//!
//! `initRail`'s callback is handed the route it is reacting to -- `{scope,
//! page, tail}` -- because back and forward move the page and what is open
//! inside it as well as the selection, and those belong to the page. A callback
//! that only cares about the selection can take no arguments and ignore it.
//!
//! `tail` is the segments after the page, and this file never looks inside it.
//! What they mean is the view's business; `app.js` registers the one function
//! that can turn what is on screen back into them.

import { $, esc, state } from "./core.js";

/// The hash segment that stands for no selection at all.
const ALL = "all";

/// The pages a hash may name, and where the segments after the page come from.
/// Both are registered by `app.js` rather than written down here: this file
/// knows about no view, and the hand-kept copy that used to sit here said
/// `["tasks", "agents"]` long after there were five views -- so three of them
/// could be written into the URL and none of the three could be read back.
let pages = [];
let tailOf = () => [];
let redirects = {};

/// True while a route read out of the URL is being applied to the page. The
/// views write the URL as they change -- a tab lighting up, a task opening --
/// and every one of those writes during an apply would push an entry for a
/// place nobody navigated to. See `applyRoute`.
let applying = false;

/// What `app.js` has to tell the router before the first hash is read: which
/// pages exist, how to ask the one on screen what its tail is, and any old page
/// names that should resolve to their replacements.
export function setRouter(r) {
  pages = r.pages;
  if (r.tailOf) tailOf = r.tailOf;
  redirects = r.redirects || {};
}

let onSelect = () => {};
/// Which views hang off each primary-menu entry -- app.js's, handed in
/// through `initRail` rather than imported, so the assignment stays the one
/// map app.js owns and this file never has to know a view by name beyond the
/// strings the hash is written in. Read by `readHash`/`writeHash` to fill in
/// a level the hash left out, or correct one that names a level nothing owns.
let levelViews = {};
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
/// Those leading segments describe where the instance happens to live, not
/// scope nesting, so they are cut off before paths are compared.
function segments(path) {
  let p = String(path ?? "");
  const root = String(state.root ?? "").replace(/\/+$/, "");
  if (root && (p === root || p.startsWith(`${root}/`))) p = p.slice(root.length);
  return p.split("/").filter(s => s !== "" && s !== ".");
}

/// Build a tree from configured scopes only. Each scope hangs from its nearest
/// configured ancestor; unmarked path segments affect containment but never
/// become rows. Segment comparison keeps `projects/factory` from swallowing
/// `projects/factory-x`.
function buildTree() {
  const root = { scope: null, path: [], children: [] };
  const nodes = state.scopes.map(scope => ({
    scope,
    path: segments(scope.path),
    children: [],
  }));
  for (const node of nodes) {
    let parent = root;
    for (const candidate of nodes) {
      if (candidate === node || candidate.path.length >= node.path.length) continue;
      if (under(node.path, candidate.path) &&
          (!parent.scope || candidate.path.length > parent.path.length)) {
        parent = candidate;
      }
    }
    parent.children.push(node);
  }
  return root;
}

function branch(node, depth) {
  if (!node.children.length) return "";
  // Discovery serves path order, which is also the roster's order.
  return `<ul>${node.children.map(c => row(c, depth)).join("")}</ul>`;
}

function row(node, depth) {
  const on = node.scope.name === state.scope ? ` aria-current="true"` : "";
  const text = node.scope.name;
  // A deep name does not fit the column and is cut short there, so the tooltip
  // carries the whole of it as well as the path it sits on.
  const head = `<button class="rail-row" style="--d:${depth}" data-scope="${esc(node.scope.name)}" title="${esc(node.scope.name)} · ${esc(node.scope.id)} · ${esc(node.scope.path)}"${on}>${esc(text)}</button>`;
  return `<li>${head}${branch(node, depth + 1)}</li>`;
}

function render() {
  const rail = $("rail");
  if (!rail) return;

  const root = buildTree();
  // An empty list means no local scope config was found or `/api/agents`
  // could not answer. Whatever configured scope is shallowest becomes a root,
  // and there can be several.
  const html = !state.scopes.length
    ? `<p class="rail-empty">No configured scopes found.</p>`
    : `
    <button class="rail-row rail-all" id="rail-all"${state.scope === null ? ` aria-current="true"` : ""}>All scopes</button>
    ${branch(root, 0)}`;

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
/// names, and a refreshed `/api/agents` can drop one.
function known(name) {
  if (!name) return null;
  return state.scopes.some(s => s.name === name) ? name : null;
}

function select(name) {
  if (name === state.scope) return;
  state.scope = name;
  render();
  writeHash();
  onSelect({ scope: state.scope, page: state.tab, tail: null });
}

export function initRail(onChange, levelViewsMap) {
  if (onChange) onSelect = onChange;
  // `rebuildRail()` calls this again on every `/api/agents` refresh without
  // repeating the map -- only the boot call carries one, and it never changes
  // afterward, so a later call simply keeps what is already here.
  if (levelViewsMap) levelViews = levelViewsMap;
  const first = !wired;
  const route = readHash();
  const before = state.scope;

  // On the first call the hash is the boot selection; on later ones -- after
  // `/api/agents` came back again -- the hash has long been normalised and what
  // matters is whether the tree still holds what is selected.
  state.scope = known(first && route ? route.scope : state.scope);
  if (first && route) state.level = route.level;
  render();

  if (first) {
    window.addEventListener("hashchange", onHashChange);
    wired = true;
    // Which page a link asked for, and what it had open, are put in place by
    // `boot` -- the views have to be wired before they can be shown, and that
    // happens after this. The rail owns the selection and stops there.
    return;
  }

  // Every later call is `loadAgents` rebuilding the rail. The URL already says
  // what is on screen and re-reading it would re-open the task just closed; all
  // that can have changed is whether the tree still holds the selection.
  if (state.scope !== before) {
    applyRoute(() => onSelect({ scope: state.scope, level: state.level, page: state.tab, tail: null }));
  } else {
    writeHash(true);
  }
}

/// The live level whose view list names `page` -- the fallback for a hash with
/// no level segment, or one that names a level nothing owns. `levelViews`
/// iterates in the order `app.js` wrote it; each view has one owner today,
/// making old bare `#scope/page` links land under the right primary entry.
function levelForPage(page) {
  for (const level of Object.keys(levelViews)) {
    if (levelViews[level].includes(page)) return level;
  }
  return null;
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

/// `#<scope>/<level>/<page>/<tail...>`, so a link names the whole of what is on
/// screen and not just the half of it the rail owns. The tail is whatever
/// `tailOf` hands back -- this file neither builds it nor reads it.
///
/// The level sits ahead of the page rather than after it because it is the
/// coarser of the two: a link read left to right narrows, and the tail belongs
/// to the page, so anything between them would separate a page from its own
/// segments.
///
/// A click is a navigation and pushes; `replace` is for the writes that only say
/// what is already on screen -- boot, correcting a hash that named a scope the
/// tree no longer has, and filling in a detail the page settled on its own, like
/// the run a task opened on -- because an entry pushed there costs a Back press
/// to get past and is not somewhere the user has been.
export function writeHash(replace) {
  // While a route is being applied the URL is the truth and the page is the one
  // catching up. `applyRoute` writes the corrected hash once, at the end.
  if (applying) return;
  // `all` is the keyword for no selection, so a scope actually called `all` has
  // to be written as something that decodes back to its name without reading as
  // the keyword. Every other name survives encodeURIComponent unchanged.
  const head = state.scope === null ? ALL
    : state.scope === ALL ? "%61ll" : encodeURIComponent(state.scope);
  const tail = tailOf().filter(seg => seg !== null && seg !== undefined && seg !== "");
  const level = state.level || levelForPage(state.tab);
  const next = `#${[head, level, state.tab, ...tail.map(encodeURIComponent)].join("/")}`;
  written = next;
  // Assigning the hash it already carries would push nothing anyway; assigning
  // a different one fires `hashchange`, and this runs on every tab switch.
  if (location.hash === next) return;
  if (replace) history.replaceState(null, "", next);
  else location.hash = next;
}

/// `{scope, level, page, tail}` for a hash that names a route, `null` for
/// anything else. Deliberately syntax only: boot reads the hash before
/// `/api/agents` has answered, so there is no tree yet to check the name
/// against, and no view has been asked whether its tail means anything.
/// `initRail` does the first, the views do the second.
///
/// Split from the left, one segment at a time. The old two-segment hash was
/// found with `lastIndexOf`, which stops being the page the moment anything
/// follows it -- and a scope name is written with `encodeURIComponent`, so a
/// name containing a slash arrives as `%2F` and never splits.
///
/// The level segment is optional, and it is told apart from the page by the
/// registered page and redirect names rather than by the level map: the route
/// vocabulary arrives before any hash is read, whereas `levelViews` only
/// arrives with the boot call to `initRail`. So a second segment that names a
/// page is a hash written before levels existed (`#<scope>/<page>/<tail...>`),
/// and anything else there is a level. Either way the level is then checked
/// against the map and replaced if it does not own the resolved page -- a link
/// to a greyed or renamed level lands on the level that does.
export function readHash() {
  const raw = location.hash.replace(/^#/, "");
  if (!raw) return null;
  const parts = raw.split("/");
  if (parts.length < 2) return null;
  const routePages = new Set([...pages, ...Object.keys(redirects)]);
  const levelled = !routePages.has(parts[1]);
  let page = levelled ? parts[2] : parts[1];
  if (!routePages.has(page)) return null;
  const levelSeg = levelled ? parts[1] : null;
  // A hash can be typed by hand, and a broken escape in one makes
  // decodeURIComponent throw. A route nobody can read is no route.
  try {
    let tail = parts.slice(levelled ? 3 : 2).map(decodeURIComponent);
    if (redirects[page]) ({ page, tail } = redirects[page](tail));
    if (!pages.includes(page)) return null;
    const level = levelSeg && levelViews[levelSeg] && levelViews[levelSeg].includes(page)
      ? levelSeg
      : levelForPage(page);
    const head = parts[0];
    return { scope: head === ALL ? null : decodeURIComponent(head), level, page, tail };
  } catch {
    return null;
  }
}

/// Put a route on the page, then write down what the page made of it. The views
/// correct what they cannot honour -- a task that has been deleted, a render
/// mode this browser has no WebGL for -- and the URL should end up naming what
/// is actually on screen rather than what was asked for.
///
/// Their writes on the way are suppressed rather than allowed and then undone:
/// showing a tab, opening a task and selecting its newest run are three writes
/// on the way to one destination, and Back should not have to walk through all
/// three to leave a link somebody pasted. What is written at the end replaces,
/// because arriving somewhere by link or by Back is not a navigation away from
/// it.
///
/// Exported because boot is a route application too -- the first hash is read
/// there, once the views can be shown.
export function applyRoute(fn) {
  applying = true;
  try {
    fn();
  } finally {
    applying = false;
  }
  writeHash(true);
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
  state.level = route.level;
  render();
  // Back and forward move the level, the page and whatever is open inside it as
  // well as the selection, and those are the page's. Whoever wired the rail up
  // gets told what the URL now says.
  applyRoute(() => onSelect({ scope: state.scope, level: route.level, page: route.page, tail: route.tail }));
}
