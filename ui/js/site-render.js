//! The lit render: a second renderer over the same site data `site.js` owns.
//! This module never reads `/api/*` or `state` itself -- everything it draws
//! comes from the `scene` object `site.js` hands to `start()`, which is the
//! same shape the isometric plan reads. Two views, one set of facts.
//!
//! Loaded lazily, and three.js itself is vendored at `ui/vendor/three.min.js`
//! (r128, unmodified) rather than fetched from a CDN, because the daemon that
//! serves this page may be running on a machine with no internet at all. If
//! the library fails to load, or the canvas has no WebGL context, `boot()`
//! returns `false` and `site.js` falls back to the plan view and says so --
//! a render mode that fails silently would be worse than one that is simply
//! not offered.
//!
//! Unlike Plan, a scope rail selection does not narrow this scene. It drives a
//! camera close-up, the selected hall's roof, and the position of its figures;
//! every other hall remains in the scene as the campus around it.

import { mb, hallSub, tickHalls, tickRoof, floorLight, STILL } from "./site.js";
import { hallFrame, interiorPlacements, siteFrame } from "./site-focus.js";

let T = null;
let scene, cam, rr, sun, hemi, amb, ray, mouse;
let cv, onSelect;
let W3 = 0, H3 = 0, groundMat = null;
let halls = {}, pick = [], lamps = [], workers = [], labels = [];
let zoneG = null;
let raf = null, running = false, t0 = 0, lastFrame = 0;
let groundMesh = null, queueSig = "", hallSig = "";
let TOD = "dusk";
let matWall, matWall2, matRoof, matTrim, matFloor, matPaint, ribT;
let getSceneFn, getPaletteFn;
let drag = null, moved = 0;
let orb = { az: -0.91, pol: 1.05, r: 40, tx: 0, ty: 0.6, tz: 0 };
let orbReady = false, focusFrameSig = "", orbTween = null;

const TODS = {
  day: { sky: "#7FA2C4", hor: "#C6D6E2", sun: "#FFF4E2", si: 2.5, hs: "#BAD1E4", hg: "#4E4E48", hi: 1.05, ai: 0.20, el: 0.92, az: 2.3, emi: 0.14, lamp: 0.0, fn: 48, ff: 150, exp: 0.98 },
  dusk: { sky: "#1E1B33", hor: "#B4653A", sun: "#FF9548", si: 1.5, hs: "#4A4770", hg: "#241C18", hi: 0.46, ai: 0.12, el: 0.20, az: 2.0, emi: 1.00, lamp: 1.0, fn: 30, ff: 112, exp: 1.06 },
  night: { sky: "#05070E", hor: "#131C33", sun: "#5876B4", si: 0.20, hs: "#141E38", hg: "#080A0E", hi: 0.26, ai: 0.06, el: 0.75, az: 1.1, emi: 1.34, lamp: 1.30, fn: 26, ff: 94, exp: 1.16 },
};

function col(hex) { return new T.Color(hex); }
function stateCol(st, PAL) { return st === "fault" ? PAL.fault : st === "wait" ? PAL.wait : st === "run" ? PAL.run : PAL.idle; }

function radialTex(inner, outer) {
  const c = document.createElement("canvas"); c.width = c.height = 128;
  const x = c.getContext("2d"), g = x.createRadialGradient(64, 64, 0, 64, 64, 64);
  g.addColorStop(0, inner); g.addColorStop(0.35, outer); g.addColorStop(1, "rgba(0,0,0,0)");
  x.fillStyle = g; x.fillRect(0, 0, 128, 128);
  return new T.CanvasTexture(c);
}
function paneTex() {
  const c = document.createElement("canvas"); c.width = 64; c.height = 32;
  const x = c.getContext("2d");
  x.fillStyle = "#0b0d10"; x.fillRect(0, 0, 64, 32);
  x.fillStyle = "#ffffff";
  for (let i = 0; i < 3; i++) x.fillRect(6 + i * 20, 7, 14, 18);
  const t = new T.CanvasTexture(c); t.wrapS = t.wrapT = T.RepeatWrapping;
  return t;
}
function ribTex() {
  const c = document.createElement("canvas"); c.width = 32; c.height = 4;
  const x = c.getContext("2d");
  x.fillStyle = "#ffffff"; x.fillRect(0, 0, 32, 4);
  x.fillStyle = "rgba(0,0,0,0.16)"; for (let i = 0; i < 32; i += 4) x.fillRect(i, 0, 2, 4);
  const t = new T.CanvasTexture(c); t.wrapS = t.wrapT = T.RepeatWrapping;
  return t;
}
function labelTex(txt, sub) {
  const pad = 12; let c = document.createElement("canvas"); let x = c.getContext("2d");
  x.font = '600 34px "IBM Plex Sans Condensed", sans-serif';
  const w = Math.ceil(x.measureText(txt).width);
  x.font = '400 22px "IBM Plex Mono", monospace';
  const w2 = sub ? Math.ceil(x.measureText(sub).width) : 0;
  c.width = Math.max(w, w2) + pad * 2; c.height = sub ? 86 : 54;
  x = c.getContext("2d");
  x.fillStyle = "rgba(8,10,14,0.72)"; x.fillRect(0, 0, c.width, c.height);
  x.fillStyle = "#ffffff"; x.textBaseline = "top";
  x.font = '600 34px "IBM Plex Sans Condensed", sans-serif';
  x.fillText(txt, pad, 9);
  if (sub) { x.fillStyle = "rgba(255,255,255,0.62)"; x.font = '400 22px "IBM Plex Mono", monospace'; x.fillText(sub, pad, 50); }
  return { tex: new T.CanvasTexture(c), ar: c.width / c.height };
}

/// Everything the plan draws as floor markings is baked here too, so a hall
/// stands on the same ground in both views. The apron has no per-directory
/// detail because Factory does not read a scope's tree -- there is nothing
/// to bake beyond the zone boundary and the aisle.
function groundTex(sceneData, PAL, bounds) {
  const PX = 40, wpx = Math.round((bounds.x1 - bounds.x0) * PX), hpx = Math.round((bounds.y1 - bounds.y0) * PX);
  const c = document.createElement("canvas"); c.width = Math.max(8, wpx); c.height = Math.max(8, hpx);
  const x = c.getContext("2d");
  const X = (g) => (g - bounds.x0) * PX, Y = (g) => (g - bounds.y0) * PX;
  x.fillStyle = PAL.ground2; x.fillRect(0, 0, c.width, c.height);
  x.strokeStyle = "rgba(0,0,0,0.13)"; x.lineWidth = 1.5;
  for (let g = Math.ceil(bounds.x0); g < bounds.x1; g += 4) { x.beginPath(); x.moveTo(X(g), 0); x.lineTo(X(g), c.height); x.stroke(); }
  for (let g = Math.ceil(bounds.y0); g < bounds.y1; g += 4) { x.beginPath(); x.moveTo(0, Y(g)); x.lineTo(c.width, Y(g)); x.stroke(); }
  const Z = sceneData.ZONE;
  if (Z) {
    x.fillStyle = "rgba(255,255,255,0.035)"; x.fillRect(X(Z.x), Y(Z.y), Z.w * PX, Z.d * PX);
    x.setLineDash([16, 12]); x.strokeStyle = PAL.paint; x.lineWidth = 4;
    x.strokeRect(X(Z.x), Y(Z.y), Z.w * PX, Z.d * PX); x.setLineDash([]);
  }
  function road(pts, wd) {
    x.strokeStyle = "rgba(0,0,0,0.30)"; x.lineWidth = wd * PX; x.lineJoin = "round"; x.lineCap = "round";
    x.beginPath(); pts.forEach((p, i) => (i ? x.lineTo(X(p[0]), Y(p[1])) : x.moveTo(X(p[0]), Y(p[1])))); x.stroke();
    x.setLineDash([18, 16]); x.strokeStyle = PAL.paint; x.lineWidth = wd * PX * 0.28;
    x.beginPath(); pts.forEach((p, i) => (i ? x.lineTo(X(p[0]), Y(p[1])) : x.moveTo(X(p[0]), Y(p[1])))); x.stroke(); x.setLineDash([]);
  }
  if (sceneData.AISLE && sceneData.AISLE.length > 1) road(sceneData.AISLE, 1.5);
  (sceneData.SPURS || []).forEach((sp) => road(sp, 0.9));
  sceneData.SITE.forEach((b) => {
    if (!b.queued.length) return;
    x.strokeStyle = "rgba(255,255,255,0.34)"; x.lineWidth = 3;
    for (let i = 0; i < b.queued.length; i++) x.strokeRect(X(b.x + 0.2 + i * 0.9), Y(b.y + b.d + 0.15), 0.8 * PX, 0.8 * PX);
  });
  const t = new T.CanvasTexture(c); t.anisotropy = rr ? rr.capabilities.getMaxAnisotropy() : 1;
  return t;
}

function glowSprite(hex, size, op) {
  const m = new T.SpriteMaterial({
    map: radialTex("rgba(255,255,255,0.95)", "rgba(255,255,255,0.30)"), color: new T.Color(hex),
    blending: T.AdditiveBlending, depthWrite: false, transparent: true, toneMapped: false, opacity: op === undefined ? 1 : op,
  });
  const s = new T.Sprite(m); s.scale.set(size, size, 1); return s;
}

function makeMaterials(PAL) {
  ribT = ribTex();
  matWall = new T.MeshStandardMaterial({ color: col(PAL.wallA), roughness: 0.86, metalness: 0.04 });
  matWall2 = new T.MeshStandardMaterial({ color: col(PAL.wallB), roughness: 0.9, metalness: 0.03 });
  matRoof = new T.MeshStandardMaterial({ color: col(PAL.roof), roughness: 0.7, metalness: 0.22, map: ribT });
  matTrim = new T.MeshStandardMaterial({ color: col(PAL.roof2), roughness: 0.55, metalness: 0.35 });
  matFloor = new T.MeshStandardMaterial({ color: col(PAL.plate), roughness: 0.92 });
  matPaint = new T.MeshBasicMaterial({ color: col(PAL.paint) });
}

function disposeGroup(group) {
  if (!group) return;
  group.traverse((o) => {
    if (o.geometry) o.geometry.dispose();
    if (o.material) {
      const mats = Array.isArray(o.material) ? o.material : [o.material];
      mats.forEach((m) => { if (m.map) m.map.dispose(); m.dispose(); });
    }
  });
  scene.remove(group);
}

let sceneGroup = null;

/// One hall. Built at the height its scope's size asks for, and then scaled
/// by how far the transition has got -- so a repository that grows a storey
/// grows it, rather than the building blinking into a new one.
///
/// Everything that grows with it goes in `shell`; everything that rides on top
/// goes in `cap`, which is moved rather than stretched so a roof does not get
/// thinner as the building gets shorter.
function hall(b, PAL) {
  const g = new T.Group(), hw = b.w, hd = b.d, hh = b.hT * 1.15;
  g.position.set(b.x, 0, b.y);
  const picks = [];
  const shell = new T.Group(), cap = new T.Group();
  g.add(shell); g.add(cap);

  // Four real walls, rather than one closed box. With the roof in place they
  // read exactly like the old solid hall; once it slides away there is no
  // hidden top face left behind to cover the shop floor and its agents.
  const wtWall = 0.16;
  const wallPanel = (geometry, x, z, material) => {
    const wall = new T.Mesh(geometry, material);
    wall.position.set(x, hh / 2, z); wall.castShadow = true; wall.receiveShadow = true; wall.userData.bid = b.id;
    shell.add(wall); picks.push(wall);
  };
  wallPanel(new T.BoxGeometry(hw, hh, wtWall), hw / 2, wtWall / 2, matWall2);
  wallPanel(new T.BoxGeometry(hw, hh, wtWall), hw / 2, hd - wtWall / 2, matWall);
  wallPanel(new T.BoxGeometry(wtWall, hh, Math.max(0.1, hd - wtWall * 2)), wtWall / 2, hd / 2, matWall2);
  wallPanel(new T.BoxGeometry(wtWall, hh, Math.max(0.1, hd - wtWall * 2)), hw - wtWall / 2, hd / 2, matWall);

  const pl = new T.Mesh(new T.BoxGeometry(hw + 0.5, 0.2, hd + 0.5), matTrim);
  pl.position.set(hw / 2, 0.1, hd / 2); pl.receiveShadow = true; pl.userData.bid = b.id;
  g.add(pl); picks.push(pl);

  // One band of windows per storey, on all four faces. How many rows there are
  // is the size signal; how far up they are lit is the activity one, applied
  // every frame in `tick` and never baked in here.
  const floors = Math.max(1, b.shape.floors);
  const bays = Math.max(2, b.shape.bays);
  const fh = hh / floors;
  const wt = paneTex();
  const winMat = () => new T.MeshStandardMaterial({
    map: wt.clone(), emissiveMap: wt.clone(), color: new T.Color("#0d0f13"),
    emissive: new T.Color("#2b3038"), emissiveIntensity: 0.12, roughness: 0.3,
  });
  const rows = [];
  for (let f = 0; f < floors; f++) {
    const meshes = [];
    const y = f * fh + fh * 0.5;
    const band = (px, pz, len, ry) => {
      const m = winMat();
      m.map.wrapS = m.map.wrapT = T.RepeatWrapping;
      m.map.repeat.set(bays, 1);
      m.emissiveMap = m.map;
      const mesh = new T.Mesh(new T.PlaneGeometry(len, Math.min(0.5, fh * 0.46)), m);
      mesh.position.set(px, y, pz); mesh.rotation.y = ry; mesh.userData.bid = b.id;
      shell.add(mesh); picks.push(mesh); meshes.push(mesh);
    };
    band(hw / 2, -0.02, hw * 0.86, Math.PI);
    band(hw / 2, hd + 0.02, hw * 0.86, 0);
    band(-0.02, hd / 2, hd * 0.86, -Math.PI / 2);
    band(hw + 0.02, hd / 2, hd * 0.86, Math.PI / 2);
    rows.push(meshes);
  }

  // The shop floor. A hall with areas gets the same tiles `site.js` treemapped
  // once, flat on the floor -- not extruded, which would read as machinery
  // rather than a plan -- so the same directory lands in the same corner in
  // both views. A hall with nothing measured gets one plain pad instead: a
  // treemap drawn from nothing would be a guess dressed up as a measurement.
  const floorTiles = [];
  if (b.floor && b.floor.tiles.length) {
    b.floor.tiles.forEach((t) => {
      const fmat = new T.MeshStandardMaterial({ color: col(PAL[t.colorKey]), roughness: 0.92 });
      const tile = new T.Mesh(new T.BoxGeometry(Math.max(t.w, 0.02), 0.09, Math.max(t.h, 0.02)), fmat);
      tile.position.set(t.x + t.w / 2, 0.27, t.y + t.h / 2); tile.receiveShadow = true; tile.userData.bid = b.id;
      g.add(tile); picks.push(tile); floorTiles.push({ mat: fmat, key: t.colorKey });
    });
  } else {
    const floor = new T.Mesh(new T.BoxGeometry(Math.max(0.1, hw - 0.6), 0.09, Math.max(0.1, hd - 0.6)), matFloor);
    floor.position.set(hw / 2, 0.27, hd / 2); floor.receiveShadow = true; floor.userData.bid = b.id;
    g.add(floor); picks.push(floor);
  }

  const slab = new T.Mesh(new T.BoxGeometry(hw + 0.3, 0.2, hd + 0.3), matRoof);
  slab.position.set(hw / 2, hh + 0.1, hd / 2); slab.castShadow = true; slab.receiveShadow = true; slab.userData.bid = b.id;
  cap.add(slab); picks.push(slab);
  const lip = new T.Mesh(new T.BoxGeometry(hw + 0.42, 0.1, hd + 0.42), matTrim);
  lip.position.set(hw / 2, hh + 0.02, hd / 2); cap.add(lip);

  const c = beaconCol(b, PAL);
  const mast = new T.Mesh(new T.CylinderGeometry(0.05, 0.05, 1.0, 6), matTrim);
  mast.position.set(0.28, hh + 0.5, 0.28); mast.castShadow = true; cap.add(mast);
  const beacon = new T.Mesh(new T.SphereGeometry(0.16, 12, 10), new T.MeshBasicMaterial({ color: new T.Color(c), toneMapped: false }));
  beacon.position.set(0.28, hh + 1.05, 0.28); cap.add(beacon);
  const beaconGlow = glowSprite(c, 1.4, 0.85); beaconGlow.position.copy(beacon.position); cap.add(beaconGlow);

  const ring = new T.Mesh(
    new T.RingGeometry(Math.max(hw, hd) * 0.62, Math.max(hw, hd) * 0.62 + 0.16, 48),
    new T.MeshBasicMaterial({ color: col(PAL.signal), transparent: true, opacity: 0, side: T.DoubleSide, depthWrite: false })
  );
  ring.rotation.x = -Math.PI / 2; ring.position.set(hw / 2, 0.24, hd / 2); g.add(ring);

  const sub = hallSub(b);
  const lt = labelTex(b.name, sub);
  const sp = new T.Sprite(new T.SpriteMaterial({ map: lt.tex, depthWrite: false, transparent: true, depthTest: false }));
  sp.scale.set(2.4 * lt.ar * 0.4, 2.4 * 0.4, 1); sp.position.set(hw / 2, hh + 1.7, hd / 2); g.add(sp); labels.push(sp);

  sceneGroup.add(g);
  // `sub` is recorded here so `applyHallFacts` does not immediately build a
  // second texture for the label it was just given.
  halls[b.id] = { b, g, shell, cap, rows, hh, beacon, beaconGlow, label: sp, sub, ring, floorTiles, picks };
}

/// What the roof light is saying, in the palette's own words. The daemon
/// decided which of the five it is (`building.rs`); this only colours it.
function beaconCol(b, PAL) {
  const key = { blocked: "wait", fault: "fault", working: "run", waiting: "idle", off: "idle" }[b.cues.beacon];
  return PAL[key] || PAL.idle;
}

/// Every mesh a click can land on, gathered from the halls standing now. Kept
/// as one flat array because that is what the raycaster wants, and rebuilt
/// rather than spliced whenever a hall comes or goes.
function repick() {
  pick = [];
  Object.values(halls).forEach((h) => h.picks.forEach((m) => pick.push(m)));
}

function dropGroup(group) {
  if (!group) return;
  group.traverse((o) => {
    if (o.geometry) o.geometry.dispose();
    if (o.material) {
      const mats = Array.isArray(o.material) ? o.material : [o.material];
      mats.forEach((m) => { if (m.map) m.map.dispose(); m.dispose(); });
    }
  });
  if (group.parent) group.parent.remove(group);
}

function figure(wk, PAL, placement) {
  const g = new T.Group();
  const c = stateCol(wk.state, PAL);
  const body = new T.Mesh(new T.CylinderGeometry(0.13, 0.17, 0.42, 8),
    new T.MeshStandardMaterial({ color: new T.Color(c), roughness: 0.7, emissive: new T.Color(c), emissiveIntensity: wk.state === "idle" ? 0.05 : 0.4 }));
  body.position.y = 0.21; body.castShadow = true; g.add(body);
  const head = new T.Mesh(new T.SphereGeometry(0.105, 10, 8), new T.MeshStandardMaterial({ color: new T.Color("#E8DCC8"), roughness: 0.8 }));
  head.position.y = 0.53; head.castShadow = true; g.add(head);
  const gl = glowSprite(c, 1.2, 0.7); gl.position.y = 0.42; g.add(gl);
  const scale = placement ? placement.scale : 1.4;
  g.scale.set(scale, scale, scale);
  g.position.set(placement ? placement.x : wk.gx, 0.3, placement ? placement.z : wk.gy);
  sceneGroup.add(g);
  return { g, wk, phase: placement ? placement.x : wk.gx };
}

/// Rebuild only the cheap agent figures. A focused hall gets a collision-free
/// grid on its floor; every other hall keeps the exterior apron positions that
/// communicate presence in the full-campus view.
function placeWorkers(sceneData, PAL) {
  workers.forEach((w) => dropGroup(w.g));
  workers = [];
  sceneData.SITE.forEach((b) => {
    const inside = b.id === sceneData.focus ? interiorPlacements(b, b.workers.length) : [];
    b.workers.forEach((wk, i) => workers.push(figure(wk, PAL, inside[i])));
  });
}

function streetLamp(x, z, PAL) {
  const g = new T.Group(); g.position.set(x, 0, z);
  const pole = new T.Mesh(new T.CylinderGeometry(0.05, 0.07, 2.9, 7), matTrim);
  pole.position.y = 1.45; pole.castShadow = true; g.add(pole);
  const head = new T.Mesh(new T.BoxGeometry(0.4, 0.12, 0.24), matTrim);
  head.position.set(0.4, 2.82, 0); g.add(head);
  const bulb = new T.Mesh(new T.SphereGeometry(0.09, 8, 6), new T.MeshBasicMaterial({ color: new T.Color("#FFD9A0"), toneMapped: false }));
  bulb.position.set(0.4, 2.74, 0); g.add(bulb);
  const gl = glowSprite("#FFC880", 2.2, 0.9); gl.position.copy(bulb.position); g.add(gl);
  const lp = new T.PointLight(new T.Color("#FFC078"), 0, 10, 2); lp.position.set(0.4, 2.72, 0); g.add(lp);
  const pool = new T.Mesh(new T.CircleGeometry(2.6, 28),
    new T.MeshBasicMaterial({ map: radialTex("rgba(255,208,150,0.85)", "rgba(255,170,90,0.22)"), transparent: true, blending: T.AdditiveBlending, depthWrite: false, opacity: 0 }));
  pool.rotation.x = -Math.PI / 2; pool.position.set(0.4, 0.03, 0); g.add(pool);
  sceneGroup.add(g);
  lamps.push({ light: lp, glow: gl, bulb, pool });
}

// --------------------------------------------------------------------- boot

export async function boot(canvas, ctx) {
  cv = canvas; getSceneFn = ctx.getScene; getPaletteFn = ctx.getPalette; onSelect = ctx.select;
  if (window.THREE) { T = window.THREE; } else {
    await new Promise((resolve, reject) => {
      const sc = document.createElement("script");
      sc.src = "/ui/vendor/three.min.js";
      sc.onload = resolve;
      sc.onerror = () => reject(new Error("three.js did not load"));
      document.head.appendChild(sc);
    });
    T = window.THREE;
  }
  if (!T) throw new Error("THREE missing after load");

  rr = new T.WebGLRenderer({ canvas: cv, antialias: true });
  if (!rr.getContext()) throw new Error("no WebGL context");
  rr.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  rr.shadowMap.enabled = true; rr.shadowMap.type = T.PCFSoftShadowMap;
  if ("outputColorSpace" in rr) rr.outputColorSpace = T.SRGBColorSpace || rr.outputColorSpace;
  else rr.outputEncoding = T.sRGBEncoding;
  rr.toneMapping = T.ACESFilmicToneMapping;

  scene = new T.Scene();
  cam = new T.PerspectiveCamera(32, 1, 0.5, 500);
  ray = new T.Raycaster(); mouse = new T.Vector2();

  hemi = new T.HemisphereLight(0xffffff, 0x444444, 1); scene.add(hemi);
  amb = new T.AmbientLight(0xffffff, 0.15); scene.add(amb);
  sun = new T.DirectionalLight(0xffffff, 1);
  sun.castShadow = true; sun.shadow.mapSize.width = sun.shadow.mapSize.height = 1536;
  const sc2 = sun.shadow.camera;
  sc2.left = -40; sc2.right = 40; sc2.top = 40; sc2.bottom = -40; sc2.near = 1; sc2.far = 200;
  sc2.updateProjectionMatrix();
  sun.shadow.bias = -0.0006;
  scene.add(sun); scene.add(sun.target);

  bindInput();
  window.addEventListener("resize", resize);
  return true;
}

function buildBounds(sceneData) {
  if (!sceneData.ZONE) return { x0: -5, x1: 5, y0: -5, y1: 5 };
  return { x0: sceneData.ZONE.x - 5, x1: sceneData.ZONE.x + sceneData.ZONE.w + 5, y0: sceneData.ZONE.y - 5, y1: sceneData.ZONE.y + sceneData.ZONE.d + 5 };
}

function moveOrbit(frame) {
  if (STILL) { Object.assign(orb, frame); orbTween = null; return; }
  orbTween = {
    from: { ...orb },
    to: frame,
    at: performance.now(),
    duration: 650,
  };
}

/// Follow the rail, but only when its focus or the geometry behind that focus
/// actually changed. Activity refreshes arrive often and must not keep
/// restarting the same camera move under a person's orbit or zoom gesture.
function syncFocus(sceneData) {
  const hall = sceneData.focus ? sceneData.byId[sceneData.focus] : null;
  const frame = hall ? hallFrame(hall, orb.az) : siteFrame(sceneData);
  const sig = `${hall ? hall.id : "all"}:${frame.tx}:${frame.ty}:${frame.tz}:${frame.r}`;
  if (sig === focusFrameSig) return;
  focusFrameSig = sig;
  moveOrbit(frame);
}

export function start(sceneData) {
  const PAL = getPaletteFn();
  makeMaterials(PAL);
  pick = []; halls = {}; lamps = []; workers = []; labels = [];
  disposeGroup(sceneGroup);
  sceneGroup = new T.Group();
  scene.add(sceneGroup);

  if (groundMat) { groundMat.map && groundMat.map.dispose(); groundMat.dispose(); }
  const bounds = buildBounds(sceneData);
  groundMat = new T.MeshStandardMaterial({ map: groundTex(sceneData, PAL, bounds), roughness: 0.96 });
  const gp = new T.Mesh(new T.PlaneGeometry(bounds.x1 - bounds.x0, bounds.y1 - bounds.y0), groundMat);
  gp.rotation.x = -Math.PI / 2; gp.position.set((bounds.x0 + bounds.x1) / 2, 0, (bounds.y0 + bounds.y1) / 2);
  gp.receiveShadow = true; sceneGroup.add(gp);

  if (sceneData.ZONE) {
    const Z = sceneData.ZONE, zg = new T.Group();
    const zm = new T.MeshBasicMaterial({ color: col(PAL.paint), transparent: true, opacity: 0.1, side: T.DoubleSide, depthWrite: false, toneMapped: false });
    [[Z.x + Z.w / 2, Z.y, Z.w, 0], [Z.x + Z.w / 2, Z.y + Z.d, Z.w, 0],
     [Z.x, Z.y + Z.d / 2, Z.d, Math.PI / 2], [Z.x + Z.w, Z.y + Z.d / 2, Z.d, Math.PI / 2]].forEach((f) => {
      const m = new T.Mesh(new T.PlaneGeometry(f[2], 1.4), zm);
      m.position.set(f[0], 0.7, f[1]); m.rotation.y = f[3]; zg.add(m);
    });
    sceneGroup.add(zg); zoneG = zg;
  }

  sceneData.SITE.forEach((b) => hall(b, PAL));
  repick();
  Object.values(halls).forEach((h) => applyHallFacts(h, PAL));
  hallSig = structureOf(sceneData);
  queueSig = queuesOf(sceneData);
  groundMesh = gp;
  if (sceneData.AISLE && sceneData.AISLE.length > 1) {
    const [a, b2] = sceneData.AISLE;
    const mid = [(a[0] + b2[0]) / 2, (a[1] + b2[1]) / 2];
    [a, mid, b2].forEach((p) => streetLamp(p[0], p[1] - 1.1, PAL));
  }
  placeWorkers(sceneData, PAL);

  if (!orbReady) {
    Object.assign(orb, siteFrame(sceneData));
    orbReady = true;
  }
  syncFocus(sceneData);

  applyTOD();
  resize();
  if (!running) { running = true; t0 = performance.now(); raf = requestAnimationFrame(tick); }
}

/// What the scene was built around, as one string. Only these move a mesh:
/// which halls stand, how tall and wide each one is, and how many storeys of
/// windows it carries. Everything else a refresh brings -- a run starting, a
/// task queued, an agent coming up -- is light and figures, which `update`
/// can carry onto the scene that is already standing.
function structureOf(sceneData) {
  return sceneData.SITE.map((b) =>
    `${b.id}:${b.shape.floors}:${b.shape.bays}:${b.w}:${b.d}:${b.hT}:${b.x}:${b.y}:${(b.floor.tiles || []).length}`
  ).join("|");
}

function queuesOf(sceneData) {
  return sceneData.SITE.map((b) => `${b.id}:${b.queued.length}`).join("|");
}

/// Take the new facts without rebuilding the city. Returns false when the site
/// itself changed shape -- a hall added, removed, re-tiered or re-measured --
/// and the caller should `start()` instead.
///
/// This is the difference between a site plan that flickers and one that does
/// not: a run starting fires an event, an event refreshes the scene, and
/// rebuilding every mesh on each one would restage the whole view several
/// times a minute and cut every transition off halfway.
export function update(sceneData) {
  if (!scene || !sceneGroup || !running) return false;
  if (structureOf(sceneData) !== hallSig) return false;
  const PAL = getPaletteFn();

  for (const b of sceneData.SITE) {
    const h = halls[b.id];
    if (!h) return false;
    h.b = b;
    applyHallFacts(h, PAL);
  }

  // Figures come and go with the work and move between apron and interior as
  // the focus changes; they are cheap, and rebuilt rather than diffed.
  placeWorkers(sceneData, PAL);

  // The crates at a hall's door are baked into the ground texture, so that one
  // is only worth redrawing when the queue actually changed.
  const q = queuesOf(sceneData);
  if (q !== queueSig && groundMesh) {
    const bounds = buildBounds(sceneData);
    if (groundMat.map) groundMat.map.dispose();
    groundMat.map = groundTex(sceneData, PAL, bounds);
    groundMat.needsUpdate = true;
    queueSig = q;
  }
  syncFocus(sceneData);
  return true;
}

/// Everything about a hall that can change without moving a mesh: what its
/// beacon says, and what its label reads.
function applyHallFacts(h, PAL) {
  const c = beaconCol(h.b, PAL);
  h.beacon.material.color.set(c);
  h.beaconGlow.material.color.set(c);
  const sub = hallSub(h.b);
  if (h.sub !== sub) {
    const lt = labelTex(h.b.name, sub);
    if (h.label.material.map) h.label.material.map.dispose();
    h.label.material.map = lt.tex;
    h.label.material.needsUpdate = true;
    h.label.scale.set(2.4 * lt.ar * 0.4, 2.4 * 0.4, 1);
    h.sub = sub;
  }
}

/// How far up a hall the lights have got, applied to the storeys themselves.
/// `floorLight` is fractional, so a floor coming on fades up rather than
/// switching, and the whole facade never changes at once.
const WINDOW_DARK = "#2b3038";
/// The same rule the plan uses (`litKey` in `site.js`): a window is warm, and
/// nothing about the kind of work changes it -- the beacon is what carries
/// that. A constant here rather than the theme's `--lit` because this is an
/// emissive light in a scene with its own hour, not a swatch on a page.
const WINDOW_WARM = "#FFC98A";
function litColour() { return WINDOW_WARM; }

function lightHall(h, tod) {
  const b = h.b, warm = litColour();
  h.rows.forEach((meshes, f) => {
    const light = floorLight(b, f) * (0.35 + 0.65 * b.glow);
    meshes.forEach((m) => {
      if (Math.abs((m.userData.light || 0) - light) < 0.01 && m.userData.warm === warm) return;
      m.userData.light = light; m.userData.warm = warm;
      m.material.emissive.set(light > 0.01 ? warm : WINDOW_DARK);
      m.material.emissiveIntensity = (0.1 + light * 1.3) * tod.emi;
    });
  });
}

export function stop() { running = false; if (raf) cancelAnimationFrame(raf); raf = null; }

export function repaint() {
  if (!scene) return;
  const PAL = getPaletteFn();
  if (matWall) matWall.color.set(PAL.wallA);
  if (matWall2) matWall2.color.set(PAL.wallB);
  if (matRoof) matRoof.color.set(PAL.roof);
  if (matTrim) matTrim.color.set(PAL.roof2);
  if (matFloor) matFloor.color.set(PAL.plate);
  if (matPaint) matPaint.color.set(PAL.paint);
  // Each floor tile keeps its own material (one colour per area, cycled from
  // the palette), so a theme change has to walk them by hand rather than
  // recolouring one shared `matFloor` the way every other surface does.
  Object.values(halls).forEach((h) => (h.floorTiles || []).forEach(({ mat, key }) => mat.color.set(PAL[key])));
  applyTOD();
}

export function setTOD(name) { TOD = TODS[name] ? name : TOD; applyTOD(); }

function applyTOD() {
  if (!scene) return;
  const p = TODS[TOD];
  scene.background = new T.Color(p.hor);
  scene.fog = new T.Fog(new T.Color(p.hor), p.fn, p.ff);
  sun.color.set(p.sun); sun.intensity = p.si;
  sun.position.set(orb.tx + Math.cos(p.az) * 50 * Math.cos(p.el), 50 * Math.sin(p.el) + 2, orb.tz + Math.sin(p.az) * 50 * Math.cos(p.el));
  sun.target.position.set(orb.tx, 0, orb.tz);
  hemi.color.set(p.hs); hemi.groundColor.set(p.hg); hemi.intensity = p.hi;
  amb.intensity = p.ai;
  rr.toneMappingExposure = p.exp;
  lamps.forEach((l) => {
    l.light.intensity = p.lamp * 2.2;
    l.glow.material.opacity = p.lamp * 0.95;
    l.pool.material.opacity = p.lamp * 0.55;
    l.bulb.material.color.set(p.lamp > 0 ? "#FFE0B0" : "#3A3A3A");
  });
  // A change of hour changes how hard every lit window burns, so the storeys
  // are re-lit rather than left at the last hour's intensity.
  Object.values(halls).forEach((h) => {
    h.rows.forEach((meshes) => meshes.forEach((m) => { m.userData.light = -1; }));
    lightHall(h, p);
  });
}

// ------------------------------------------------------------------- input

function bindInput() {
  cv.addEventListener("pointerdown", (e) => {
    cv.setPointerCapture && cv.setPointerCapture(e.pointerId);
    orbTween = null;
    drag = { x: e.clientX, y: e.clientY, pan: e.shiftKey || e.button === 2 }; moved = 0;
    cv.classList.add("drag");
  });
  cv.addEventListener("pointermove", (e) => {
    if (drag) {
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      drag.x = e.clientX; drag.y = e.clientY; moved += Math.abs(dx) + Math.abs(dy);
      if (drag.pan) {
        const k = orb.r * 0.0016;
        orb.tx -= (dx * Math.cos(orb.az) - dy * Math.sin(orb.az)) * k;
        orb.tz -= (dx * Math.sin(orb.az) + dy * Math.cos(orb.az)) * k;
      } else {
        orb.az -= dx * 0.006;
        orb.pol = Math.max(0.16, Math.min(1.42, orb.pol - dy * 0.005));
      }
    }
  });
  function up(e) { if (drag && moved < 5) clickAt(e); drag = null; cv.classList.remove("drag"); }
  cv.addEventListener("pointerup", up);
  cv.addEventListener("pointercancel", () => { drag = null; cv.classList.remove("drag"); });
  cv.addEventListener("wheel", (e) => {
    e.preventDefault(); orbTween = null;
    orb.r = Math.max(9, Math.min(140, orb.r * Math.exp(e.deltaY * 0.0012)));
  }, { passive: false });
  cv.addEventListener("contextmenu", (e) => e.preventDefault());
}

function castAt(e) {
  const r = cv.getBoundingClientRect();
  mouse.x = ((e.clientX - r.left) / r.width) * 2 - 1;
  mouse.y = -((e.clientY - r.top) / r.height) * 2 + 1;
  ray.setFromCamera(mouse, cam);
  const hit = ray.intersectObjects(pick, false);
  return hit.length ? hit[0].object.userData.bid : null;
}
function clickAt(e) {
  const id = castAt(e);
  if (id && onSelect) onSelect(id);
}

export function zoom(factor) { orbTween = null; orb.r = Math.max(9, Math.min(140, orb.r / factor)); }
export function fit() {
  const s = getSceneFn();
  orbTween = null;
  Object.assign(orb, siteFrame(s));
}

function resize() {
  if (!cv || !cv.clientWidth) return;
  const w = cv.clientWidth, h = cv.clientHeight;
  W3 = w; H3 = h;
  rr.setSize(w, h, false);
  cam.aspect = w / h; cam.updateProjectionMatrix();
}

function tick(now) {
  if (!running) return;
  raf = requestAnimationFrame(tick);
  if (!cv.clientWidth) return;
  if (Math.abs(cv.clientWidth - W3) > 1 || Math.abs(cv.clientHeight - H3) > 1) resize();
  const t = Math.max(0, (now - t0) / 1000);
  const s = getSceneFn(), sel = s.getSel();
  // Both views draw the same halls, so only the one on screen eases them --
  // and it has to, or a hall would sit half-grown for as long as this mode is
  // the one being looked at.
  const dt = lastFrame ? (now - lastFrame) / 1000 : 0;
  tickHalls(dt);
  s.SITE.forEach((b) => tickRoof(b, dt));
  lastFrame = now;
  const tod = TODS[TOD];

  if (orbTween) {
    const k = Math.min(1, (now - orbTween.at) / orbTween.duration);
    const ease = 1 - Math.pow(1 - k, 3);
    for (const key of ["tx", "ty", "tz", "r", "az", "pol"]) {
      orb[key] = orbTween.from[key] + (orbTween.to[key] - orbTween.from[key]) * ease;
    }
    if (k >= 1) orbTween = null;
  }

  Object.keys(halls).forEach((id) => {
    const h = halls[id], b = h.b;
    h.ring.material.opacity = (id === sel ? 0.85 : 0) * (0.7 + 0.3 * Math.sin(t * 3));
    // Full-site labels are sized to read from the fitted camera and become
    // billboards across the close-up. The selected name is already fixed in
    // the scope rail and inspector, so clear the view into its open roof while
    // focused and restore every label with the campus framing.
    h.label.visible = s.layers.labels && !s.focus;

    // Where the building has got to between the height it had and the one its
    // size now asks for. The shell stretches; the roof and its mast ride on
    // top rather than stretching with it.
    const k = Math.max(0.04, (b.h * 1.15) / h.hh);
    h.shell.scale.y = k;
    h.cap.position.set(
      -b.open * Math.min(2.2, b.w * 0.28),
      h.hh * (k - 1) + b.open * Math.min(3.2, h.hh * 0.55 + 1.0),
      -b.open * Math.min(1.8, b.d * 0.24),
    );
    h.label.position.y = h.hh * k + 1.7;

    lightHall(h, tod);

    // A run that is going makes the beacon beat, and more of them make it beat
    // faster -- `pulse_ms` is the daemon's number, so the plan and the render
    // beat together. A blocked run or a failed agent holds it bright and
    // still: neither is progress, and a blinking light would say it was.
    const period = STILL ? 0 : b.cues.pulse_ms;
    if (period > 0) {
      const beat = 0.55 + 0.45 * Math.sin((t * 2 * Math.PI * 1000) / period + b.x + b.y);
      h.beaconGlow.material.opacity = 0.35 + 0.65 * beat;
      const sc = 1.2 + 0.5 * beat; h.beaconGlow.scale.set(sc, sc, 1);
    } else {
      const still = b.cues.beacon === "off" ? 0.15 : b.cues.beacon === "waiting" ? 0.45 : 0.9;
      h.beaconGlow.material.opacity = still;
      h.beaconGlow.scale.set(1.3, 1.3, 1);
    }
  });
  workers.forEach((w) => {
    w.g.rotation.y = Math.sin(t * 0.6 + w.phase) * 0.3;
    w.g.position.y = 0.3 + (w.wk.state !== "idle" ? Math.abs(Math.sin(t * 1.6 + w.phase)) * 0.04 : 0);
  });

  const sp = Math.sin(orb.pol), cp = Math.cos(orb.pol);
  cam.position.set(orb.tx + orb.r * sp * Math.cos(orb.az), orb.ty + orb.r * cp, orb.tz + orb.r * sp * Math.sin(orb.az));
  cam.lookAt(orb.tx, orb.ty + 1.1, orb.tz);
  rr.render(scene, cam);
}
