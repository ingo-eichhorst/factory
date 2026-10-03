import test from "node:test";
import assert from "node:assert/strict";
import { enrichReachability } from "../../examples/dependency-reachability.mjs";

const a = "pkg:cargo/demo-lib@1.2.3", b = "pkg:cargo/another-lib@2.0.0";
const sbom = { bomFormat: "CycloneDX", components: [
  { "bom-ref": a, purl: a }, { "bom-ref": b, purl: b },
] };
function vulnerabilities() {
  return { bomFormat: "CycloneDX", specVersion: "1.6", vulnerabilities: [{
    id: "CVE-TEST", affects: [{ ref: a }, { ref: b }],
    ratings: [{ severity: "critical" }], properties: [{ name: "factory:kev", value: "true" }],
  }] };
}
function osv(called, extra = {}) {
  return { results: [{ source: { path: "Cargo.lock" }, packages: [{
    package: { name: "demo-lib", version: "1.2.3", ecosystem: "crates.io", ...extra },
    vulnerabilities: [{ id: "RUSTSEC-TEST", aliases: ["CVE-TEST"] }],
    groups: [{ ids: ["RUSTSEC-TEST"], experimental_analysis: { "RUSTSEC-TEST": { called } } }],
  }] }] };
}
function property(doc, name) {
  return JSON.parse(doc.vulnerabilities[0].properties.find((p) => p.name === name).value);
}

test("OSV aliases and exact package/version bind claims separately per affected ref", () => {
  const output = enrichReachability(sbom, vulnerabilities(), osv(true));
  assert.deepEqual(property(output, "factory:reachability"), { [a]: "reachable", [b]: "unknown" });
  const evidence = property(output, "factory:reachability-evidence");
  assert.deepEqual(evidence[a].package, ["crates.io", "demo-lib", "1.2.3"]);
  assert.equal(evidence[a].groups[0].experimental_analysis["RUSTSEC-TEST"].called, true);
  assert.match(property(output, "factory:reachability-detail")[a], /OSV-Scanner.*reachable/);
});

test("uncalled evidence is only a scanner claim, while unsupported/nonboolean evidence stays unknown", () => {
  for (const [called, value] of [[false, "unreachable"], [undefined, "unknown"], ["false", "unknown"], [null, "unknown"]]) {
    const output = enrichReachability(sbom, vulnerabilities(), osv(called));
    assert.equal(property(output, "factory:reachability")[a], value);
    assert.equal(output.vulnerabilities[0].analysis, undefined);
  }
});

test("wrong name/version/ecosystem and unmatched vulnerability are never accepted", () => {
  for (const extra of [{ version: "9.0" }, { name: "foreign" }, { ecosystem: "Go" }]) {
    assert.equal(property(enrichReachability(sbom, vulnerabilities(), osv(true, extra)), "factory:reachability")[a], "unknown");
  }
  const wrong = vulnerabilities(); wrong.vulnerabilities[0].id = "CVE-OTHER";
  assert.equal(property(enrichReachability(sbom, wrong, osv(true)), "factory:reachability")[a], "unknown");
});

test("evidence enrichment does not mutate inputs or authored VEX and exploit/severity data", () => {
  const document = vulnerabilities();
  document.vulnerabilities[0].analysis = { state: "exploitable", detail: "authored reasoning", justification: "code_reachable", response: ["update"] };
  const before = structuredClone(document);
  const output = enrichReachability(sbom, document, osv(false));
  assert.deepEqual(document, before);
  assert.deepEqual(output.vulnerabilities[0].analysis, before.vulnerabilities[0].analysis);
  assert.deepEqual(output.vulnerabilities[0].affects, before.vulnerabilities[0].affects);
  assert.deepEqual(output.vulnerabilities[0].ratings, before.vulnerabilities[0].ratings);
  assert.deepEqual(output.vulnerabilities[0].properties.find((p) => p.name === "factory:kev"), { name: "factory:kev", value: "true" });
});

test("a positive call wins mixed groups, while unknown analysis prevents an all-uncalled claim", () => {
  const source = osv(false);
  const row = source.results[0].packages[0];
  row.groups.push({ ids: ["CVE-TEST"], experimental_analysis: { "CVE-TEST": {} } });
  assert.equal(property(enrichReachability(sbom, vulnerabilities(), source), "factory:reachability")[a], "unknown");
  row.groups[1].experimental_analysis["CVE-TEST"].called = true;
  assert.equal(property(enrichReachability(sbom, vulnerabilities(), source), "factory:reachability")[a], "reachable");
});

test("malformed scanner output fails rather than fabricating safe evidence", () => {
  assert.throws(() => enrichReachability(sbom, vulnerabilities(), {}), /results array/);
  assert.throws(() => enrichReachability(sbom, vulnerabilities(), { results: [{}] }), /packages array/);
  assert.throws(() => enrichReachability({}, vulnerabilities(), osv(true)), /CycloneDX/);
});

test("purl percent-encoded identities work, but a missing purl never falls back to a guessed component name", () => {
  const reference = "uuid-library";
  const inventory = { bomFormat: "CycloneDX", components: [{ "bom-ref": reference, name: "demo-lib", version: "1.2.3" }] };
  const document = vulnerabilities(); document.vulnerabilities[0].affects = [{ ref: reference }];
  assert.equal(property(enrichReachability(inventory, document, osv(true)), "factory:reachability")[reference], "unknown");
  inventory.components[0].purl = "pkg:cargo/demo%2Dlib@1.2.3";
  assert.equal(property(enrichReachability(inventory, document, osv(true)), "factory:reachability")[reference], "reachable");
});

test("native OSV v2 snake-case analysis and group aliases join a CVE", () => {
  const native = { results: [{ source: { path: "go.mod", type: "lockfile" }, packages: [{
    package: { name: "golang.org/x/text", version: "0.3.0", ecosystem: "Go" },
    groups: [{ ids: ["GO-2021-0113"], aliases: ["GO-2021-0113", "CVE-2021-38561"],
      experimental_analysis: { "GO-2021-0113": { called: true, unimportant: false } } }],
  }] }] };
  const ref = "pkg:golang/golang.org/x/text@0.3.0";
  const inventory = { bomFormat: "CycloneDX", components: [{ "bom-ref": ref, purl: ref }] };
  const document = vulnerabilities();
  document.vulnerabilities[0].id = "CVE-2021-38561";
  document.vulnerabilities[0].affects = [{ ref }];
  assert.equal(property(enrichReachability(inventory, document, native), "factory:reachability")[ref], "reachable");
});

test("a matching group without analysis prevents an all-uncalled claim", () => {
  const source = osv(false);
  source.results[0].packages[0].groups.push({ ids: ["CVE-TEST"] });
  assert.equal(property(enrichReachability(sbom, vulnerabilities(), source), "factory:reachability")[a], "unknown");
});

test("transitive advisory aliases are joined regardless of input order", () => {
  const source = osv(true);
  source.results[0].packages[0].vulnerabilities = [
    { id: "GHSA-MIDDLE", aliases: ["CVE-TEST"] },
    { id: "RUSTSEC-TEST", aliases: ["GHSA-MIDDLE"] },
  ];
  assert.equal(property(enrichReachability(sbom, vulnerabilities(), source), "factory:reachability")[a], "reachable");
});
