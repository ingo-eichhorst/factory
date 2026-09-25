//! Pure shaping for L2 Dependencies. The daemon derives truth (including
//! status); this module only groups and formats that answer for the view.

const STATUS_ORDER = { open: 0, assessed: 1, stale: 2, resolved: 3 };
const SEVERITY_ORDER = { critical: 0, high: 1, medium: 2, low: 3, unknown: 4 };

export function shapeFindings(findings) {
  return [...(findings || [])].sort((a, b) =>
    (STATUS_ORDER[a.status] ?? 9) - (STATUS_ORDER[b.status] ?? 9) ||
    (SEVERITY_ORDER[a.severity] ?? 9) - (SEVERITY_ORDER[b.severity] ?? 9) ||
    String(a.id).localeCompare(String(b.id)) ||
    String(a.affected?.name).localeCompare(String(b.affected?.name))
  );
}

export function findingCounts(findings) {
  const counts = { open: 0, assessed: 0, resolved: 0, stale: 0 };
  for (const finding of findings || []) {
    if (Object.hasOwn(counts, finding.status)) counts[finding.status]++;
  }
  return counts;
}

export function affectedLabel(affected) {
  if (!affected) return "—";
  return affected.version ? `${affected.name}@${affected.version}` : (affected.name || affected.bom_ref || "—");
}

export function pathLabel(affected) {
  return affected?.path?.length ? affected.path.join(" → ") : affectedLabel(affected);
}

export function scanLabel(scan) {
  if (!scan?.attachment) return "—";
  const tool = [scan.tool, scan.tool_version].filter(Boolean).join(" ");
  const at = scan.scan_time || scan.attachment.attached_at;
  return [tool, `attempt ${scan.attachment.attempt}`, at].filter(Boolean).join(" · ");
}

export function exploitSignals(finding) {
  const signals = [];
  if (finding?.kev) signals.push("KEV");
  if (finding?.euvd) signals.push("EUVD");
  if (finding?.epss !== null && finding?.epss !== undefined) signals.push(`EPSS ${finding.epss}`);
  return signals;
}

export function serviceTarget(service) {
  if (!service) return "—";
  if (service.endpoints?.length) return service.endpoints.join(", ");
  return service.path || "—";
}

export function credentialState(row) {
  if (!row?.credential) return "none";
  if (row.credential_present === true) return "present";
  if (row.credential_present === false) return "absent";
  return "unchecked";
}
