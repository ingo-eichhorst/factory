#!/usr/bin/env node
// Workflow-side OSV v2 -> CycloneDX evidence. Never creates a VEX verdict.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ECOSYSTEMS = { cargo: "crates.io", golang: "Go", npm: "npm", pypi: "PyPI" };

function packageKey(component, reference) {
  const purl = component?.purl || reference;
  const match = /^pkg:([^/]+)\/(.+)@([^?#]+)(?:[?#].*)?$/.exec(purl || "");
  if (!match || !ECOSYSTEMS[match[1]]) return null;
  try {
    return JSON.stringify([ECOSYSTEMS[match[1]], decodeURIComponent(match[2]), decodeURIComponent(match[3])]);
  } catch {
    return null;
  }
}

function componentsByRef(sbom) {
  const out = new Map();
  function visit(component) {
    if (component?.["bom-ref"]) out.set(component["bom-ref"], component);
    for (const child of component?.components || []) visit(child);
  }
  visit(sbom.metadata?.component);
  for (const component of sbom.components || []) visit(component);
  return out;
}

function groupsByPackage(osv) {
  if (!Array.isArray(osv?.results)) throw new Error("OSV output must contain a results array");
  const out = new Map();
  for (const result of osv.results) {
    if (!Array.isArray(result.packages)) throw new Error("OSV result must contain a packages array");
    for (const row of result.packages) {
      const pkg = row.package;
      if (!pkg || ![pkg.ecosystem, pkg.name, pkg.version].every((v) => typeof v === "string" && v)) continue;
      const key = JSON.stringify([pkg.ecosystem, pkg.name, pkg.version]);
      const groups = out.get(key) || [];
      for (const group of row.groups || []) {
        const ids = new Set([...(group.ids || []), ...(group.aliases || [])].filter((id) => typeof id === "string"));
        // A CVE may be represented by a GHSA/RUSTSEC id in the call group.
        let changed = true;
        while (changed) {
          const previous = ids.size;
          for (const vuln of row.vulnerabilities || []) {
            const aliases = [vuln.id, ...(vuln.aliases || [])].filter((id) => typeof id === "string");
            if (aliases.some((id) => ids.has(id))) for (const id of aliases) ids.add(id);
          }
          changed = previous !== ids.size;
        }
        groups.push({ ids, analysis: group.experimental_analysis || {}, source: result.source || null });
      }
      out.set(key, groups);
    }
  }
  return out;
}

function report(groups, ids) {
  const matching = groups.filter((group) => [...group.ids].some((id) => ids.has(id)));
  const byGroup = matching.map((group) => Object.entries(group.analysis)
    .filter(([id]) => group.ids.has(id)).map(([id, value]) => ({ id, called: value?.called })));
  const analyses = byGroup.flat();
  let value = "unknown";
  if (analyses.some((a) => a.called === true)) value = "reachable";
  else if (byGroup.length && byGroup.every((items) => items.length && items.every((a) => a.called === false))) value = "unreachable";
  return {
    value,
    evidence: matching.map((group) => ({ ids: [...group.ids], experimental_analysis: group.analysis, source: group.source })),
    detail: analyses.length
      ? `OSV-Scanner call analysis reports ${value}: ${analyses.map((a) => `${a.id} called=${typeof a.called === "boolean" ? a.called : "unknown"}`).join(", ")}. Scanner evidence only; not an exploitability or VEX verdict.`
      : "OSV-Scanner call analysis is unknown: no matching function-analysis evidence for this exact affected package/version. Not an exploitability or VEX verdict.",
  };
}

function replaceProperty(properties, name, value) {
  return [...properties.filter((p) => p.name !== name), { name, value: JSON.stringify(value) }];
}

export function enrichReachability(sbom, vulnerabilities, osv) {
  if (sbom?.bomFormat !== "CycloneDX" || vulnerabilities?.bomFormat !== "CycloneDX"
    || !Array.isArray(vulnerabilities.vulnerabilities)) throw new Error("expected separate CycloneDX SBOM and vulnerability documents");
  const components = componentsByRef(sbom);
  const groups = groupsByPackage(osv);
  const result = structuredClone(vulnerabilities);
  for (const vuln of result.vulnerabilities) {
    const ids = new Set([vuln.id, ...(vuln.references || []).map((ref) => ref.id)].filter((id) => typeof id === "string"));
    const values = Object.create(null), details = Object.create(null), evidence = Object.create(null);
    for (const affect of vuln.affects || []) {
      if (typeof affect.ref !== "string") continue;
      const key = packageKey(components.get(affect.ref), affect.ref);
      const claim = report(key ? groups.get(key) || [] : [], ids);
      values[affect.ref] = claim.value;
      details[affect.ref] = claim.detail;
      evidence[affect.ref] = { tool: "OSV-Scanner", package: key ? JSON.parse(key) : null, groups: claim.evidence };
    }
    let properties = vuln.properties || [];
    if (!Array.isArray(properties)) throw new Error("CycloneDX vulnerability properties must be an array");
    properties = replaceProperty(properties, "factory:reachability", values);
    properties = replaceProperty(properties, "factory:reachability-detail", details);
    vuln.properties = replaceProperty(properties, "factory:reachability-evidence", evidence);
    // All ratings, affects, exploit flags and authored analysis are untouched.
  }
  return result;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 5) throw new Error("usage: dependency-reachability.mjs SBOM VULNERABILITIES OSV_JSON");
    const [sbom, vulnerabilities, osv] = process.argv.slice(2).map((path) => JSON.parse(readFileSync(path, "utf8")));
    process.stdout.write(JSON.stringify(enrichReachability(sbom, vulnerabilities, osv), null, 2) + "\n");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
