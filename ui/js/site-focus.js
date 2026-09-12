//! Pure layout decisions for the 3D site's scope focus. Keeping these away
//! from three.js and the DOM makes the distinction explicit: Plan still
//! filters to the scope rail, while Render keeps the whole site and treats
//! that same selection as a camera/interior focus.

/// Plan owns the old scoped subset. Render always owns the complete campus,
/// so choosing a scope cannot make its neighbours disappear.
export function scopesForMode(scopes, renderMode, inScope) {
  return renderMode === 1 ? scopes.slice() : scopes.filter((scope) => inScope(scope.name));
}

/// A rail selection only focuses a real hall in Render mode. `null` covers
/// All scopes as well as a selection that vanished between API refreshes.
export function focusForMode(byId, selectedScope, renderMode) {
  if (renderMode !== 1 || !selectedScope) return null;
  return byId[selectedScope] || null;
}

/// Arrange the figures on a hall's shop floor. Positions stay inside an inset
/// rectangle and the figures shrink with their cells when a scope has many
/// present agents, rather than spilling through a wall or sitting on top of
/// one another.
export function interiorPlacements(hall, count) {
  if (count <= 0) return [];
  const inset = Math.min(0.7, hall.w * 0.18, hall.d * 0.18);
  const width = Math.max(0.2, hall.w - inset * 2);
  const depth = Math.max(0.2, hall.d - inset * 2);
  const columns = Math.min(count, Math.max(1, Math.ceil(Math.sqrt(count * width / depth))));
  const rows = Math.ceil(count / columns);
  const cellW = width / columns, cellD = depth / rows;
  const scale = Math.min(1.4, Math.min(cellW, cellD) * 1.6);

  return Array.from({ length: count }, (_, i) => {
    const row = Math.floor(i / columns), column = i % columns;
    const inRow = Math.min(columns, count - row * columns);
    const rowInset = (width - inRow * cellW) / 2;
    return {
      x: hall.x + inset + rowInset + (column + 0.5) * cellW,
      z: hall.y + inset + (row + 0.5) * cellD,
      scale,
    };
  });
}

/// The ordinary fitted campus camera. This remains a stable destination so
/// clearing a focus has somewhere deterministic to return to.
export function siteFrame(sceneData) {
  if (!sceneData.ZONE) return { tx: 0, ty: 0.6, tz: 0, r: 18, az: -0.91, pol: 1.05 };
  return {
    tx: sceneData.ZONE.x + sceneData.ZONE.w / 2,
    ty: 0.6,
    tz: sceneData.ZONE.y + sceneData.ZONE.d / 2,
    r: Math.max(18, Math.max(sceneData.ZONE.w, sceneData.ZONE.d) * 0.9),
    az: -0.91,
    pol: 1.05,
  };
}

/// Close enough to read the floor, and high enough to look through the open
/// roof. The radius follows the hall's diagonal so the largest building is
/// framed rather than clipped while smaller halls still get a real close-up.
export function hallFrame(hall, azimuth = -0.91) {
  return {
    tx: hall.x + hall.w / 2,
    ty: 0,
    tz: hall.y + hall.d / 2,
    r: Math.max(12, Math.hypot(hall.w, hall.d) * 1.3),
    az: azimuth,
    pol: 0.82,
  };
}
