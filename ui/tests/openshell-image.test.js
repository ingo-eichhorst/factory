import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const script = readFileSync(new URL("../../examples/openshell/build-image.sh", import.meta.url), "utf8");
const dockerfile = readFileSync(new URL("../../examples/openshell/Dockerfile", import.meta.url), "utf8");

test("both OpenShell image modes share an immutable base and aarch64 Linux architecture", () => {
  const pinned = /ghcr\.io\/nvidia\/openshell-community\/sandboxes\/base@sha256:[a-f0-9]{64}/;
  assert.equal(script.match(pinned)?.[0], dockerfile.match(pinned)?.[0]);
  assert.ok(script.match(pinned));
  assert.doesNotMatch(dockerfile, /base:latest/);
  assert.match(script, /docker build --platform linux\/arm64/);
  assert.match(script, /crane export --platform linux\/arm64/);
});

test("image tool downloads refuse insecure initial protocols and redirects", () => {
  const curls = script.split("\n").filter((line) => line.startsWith("curl "));
  assert.equal(curls.length, 2);
  for (const line of curls) {
    assert.match(line, /--proto '=https'/);
    assert.match(line, /--proto-redir '=https'/);
    assert.match(line, /--tlsv1\.2/);
  }
});

test("downloaded image tools must match release digests, including version overrides", () => {
  assert.match(script, /HERDR_SHA256 is required for a custom HERDR_VERSION/);
  assert.match(script, /JQ_SHA256 is required for a custom JQ_VERSION/);
  assert.match(script, /verify_digest "\$stage\/usr\/local\/bin\/herdr" "\$herdr_sha256"/);
  assert.match(script, /verify_digest "\$stage\/usr\/local\/bin\/jq" "\$jq_sha256"/);
  assert.match(script, /SHA-256 mismatch/);
});
