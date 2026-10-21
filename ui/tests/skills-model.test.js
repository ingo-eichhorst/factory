import test from "node:test";
import assert from "node:assert/strict";
import { SKILL_PROPOSALS_PATH, isSkillAction, skillActionRequest, skillInboxRows } from "../js/skills-model.js";

const proposal = (patch = {}) => ({
  id: "p-1", scope: "radar", skill: "conference-radar-tracking", path: "skill/", state: "open",
  summary: "Check sessionize's JSON API before scraping", filed_at: "2026-10-21T10:00:00Z",
  origin: { task_id: "t-1", run_id: "r-1", agent: "scout" },
  changes: [{ path: "SKILL.md", kind: "changed" }, { path: "references/old.md", kind: "removed" }],
  diff: "--- a/SKILL.md\n+++ b/SKILL.md\n@@ -1 +1 @@\n-old\n+new\n", ...patch,
});

test("open skill proposals become Inbox rows with their diff and the owner's two answers", () => {
  const now = Date.parse("2026-10-21T10:05:00Z");
  const rows = skillInboxRows({ proposals: [proposal({ id: "p-2", filed_at: "2026-10-21T10:01:00Z" }), proposal(), proposal({ id: "p-3", state: "accepted" })] }, now);
  assert.deepEqual(rows.map(r => r.proposal), ["p-1", "p-2"], "oldest first, decided ones left out");
  const [row] = rows;
  assert.equal(row.kind, "skill_proposal");
  assert.equal(row.title, "skill conference-radar-tracking");
  assert.equal(row.age, 300);
  assert.equal(row.scope, "radar");
  assert.equal(row.task_id, "t-1");
  assert.equal(row.run_id, "r-1");
  assert.match(row.reason, /before scraping · SKILL\.md, −references\/old\.md · from scout/);
  assert.match(row.diff, /\+new/);
  assert.deepEqual(row.actions, ["accept_skill", "reject_skill"]);
  assert.deepEqual(skillInboxRows(null), [], "an unreadable source adds nothing");
  assert.deepEqual(skillInboxRows({ proposals: [] }), []);
  assert.equal(SKILL_PROPOSALS_PATH, "/api/skills/proposals?state=open");
});

test("accept and reject go to the proposal, reject with its reason", () => {
  assert.deepEqual(skillActionRequest("accept_skill", "p/1"), { method: "POST", path: "/api/skills/proposals/p%2F1/accept", body: {} });
  assert.deepEqual(skillActionRequest("reject_skill", "p-1", { reason: "too broad" }), { method: "POST", path: "/api/skills/proposals/p-1/reject", body: { reason: "too broad" } });
  assert.equal(skillActionRequest("approve", "p-1"), null);
  assert.ok(isSkillAction("accept_skill") && isSkillAction("reject_skill"));
  assert.ok(!isSkillAction("approve") && !isSkillAction("toString"));
});
