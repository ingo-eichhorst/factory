//! Pure helpers for L1 Doctor's Factory dependency evidence.

export function doctorFailure(error) {
  const message = error && error.message ? String(error.message) : String(error ?? "");
  return /^404\b/.test(message) ? "unavailable" : "error";
}

export function statusSummary(status) {
  switch (status) {
    case "current": return { level: "ok", label: "Current", detail: "The installed binaries match the newest scanned build." };
    case "behind": return { level: "warn", label: "Behind", detail: "The installed binaries do not match the newest scanned build." };
    default: return { level: "bad", label: "Missing evidence", detail: "Doctor needs both a build and an installed-binary scan with product identity." };
  }
}

export function identityText(document) {
  const identity = document?.sbom?.identity;
  if (!identity) return "identity not recorded";
  return `v${identity.version} · ${identity.git_sha}`;
}

export function scanAt(document) {
  return document?.sbom?.scan_time || document?.sbom?.attachment?.attached_at || null;
}

export function scanAge(now, document) {
  const at = scanAt(document);
  const end = Date.parse(now);
  const start = Date.parse(at);
  if (!Number.isFinite(end) || !Number.isFinite(start)) return "scan time unknown";
  const seconds = Math.max(0, Math.floor((end - start) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}

export function findingSummary(findings) {
  const rows = findings || [];
  return {
    open: rows.filter((finding) => finding.status === "open").length,
    assessed: rows.filter((finding) => finding.status === "assessed").length,
    total: rows.length,
  };
}

/// One OpenShell gateway as L1 Doctor shows it (#234): whether it answers,
/// and the last time the daemon found it down and started it -- which is
/// worth seeing even once it is back.
export function gatewaySummary(row) {
  const connected = String(row?.status || "").toLowerCase() === "connected";
  const facts = [row?.server, row?.version ? `v${row.version}` : null].filter(Boolean);
  return {
    level: connected ? "ok" : "bad",
    label: connected ? "connected" : (row?.status || "unknown"),
    name: row?.gateway || "(active)",
    facts: facts.join(" · "),
    started: startedLine(row),
    detail: row?.detail || null,
  };
}

function startedLine(row) {
  if (!row?.started_at) return null;
  const how = row.started_with ? " with " + row.started_with : "";
  return "started by Factory at " + row.started_at + how;
}
