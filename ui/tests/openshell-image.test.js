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
  assert.equal(curls.length, 3, "Zig, herdr and jq are all HTTPS-only");
  for (const line of curls) {
    assert.match(line, /--proto '=https'/);
    assert.match(line, /--proto-redir '=https'/);
    assert.match(line, /--tlsv1\.2/);
  }
});

test("downloaded image tools must match release digests, including version overrides", () => {
  assert.match(script, /HERDR_SHA256 is required for a custom HERDR_VERSION/);
  assert.match(script, /JQ_SHA256 is required for a custom JQ_VERSION/);
  assert.match(script, /ZIG_SHA256 is required for a custom ZIG_VERSION/);
  assert.match(script, /verify_digest "\$work\/zig\.tar\.xz" "\$zig_sha256"/);
  assert.match(script, /verify_digest "\$stage\/usr\/local\/bin\/herdr" "\$herdr_sha256"/);
  assert.match(script, /verify_digest "\$stage\/usr\/local\/bin\/jq" "\$jq_sha256"/);
  assert.match(script, /SHA-256 mismatch/);
});

test("the bundled C compiler uses pinned official host SDKs and target-scoped tools", () => {
  assert.match(script, /zig_version=\$\{ZIG_VERSION:-0\.14\.1\}/);
  for (const digest of [
    "39f3dc5e79c22088ce878edc821dedb4ca5a1cd9f5ef915e9b3cc3053e8faefa",
    "b0f8bdfb9035783db58dd6c19d7dea89892acc3814421853e5752fe4573e5f43",
    "f7a654acc967864f7a050ddacfaa778c7504a0eca8d2b678839c21eea47c992b",
    "24aeeec8af16c381934a6cd7d95c807a8cb2cf7df9fa40d359aa884195c4716c",
  ]) assert.ok(script.includes(digest));
  assert.match(script, /CC_aarch64_unknown_linux_musl="\$work\/cc" AR_aarch64_unknown_linux_musl="\$work\/ar"/);
  assert.match(script, /cc -target aarch64-linux-musl/);
  assert.match(script, /--target=aarch64-unknown-linux-musl\) ;;/);
  assert.match(script, /--connect-timeout 30 --max-time 1800/);
  assert.doesNotMatch(script, /brew install zig|rustup toolchain install/);
});
