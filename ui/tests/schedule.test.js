import test from "node:test";
import assert from "node:assert/strict";

import { parseSchedule, scheduleLabel, scheduleText, scheduleZone } from "../js/schedule.js";

test("a cron schedule from before timezones reads exactly as it did", () => {
  const old = { cron: "0 7 * * 1" };
  assert.equal(scheduleText(old), "0 7 * * 1");
  assert.equal(scheduleZone(old), "");
  assert.equal(scheduleLabel(old), "cron 0 7 * * 1");
});

test("a zoned cron schedule fills both boxes and names its zone in a list", () => {
  const zoned = { cron: { expr: "0 9 * * 1", timezone: "Europe/Berlin" } };
  assert.equal(scheduleText(zoned), "0 9 * * 1");
  assert.equal(scheduleZone(zoned), "Europe/Berlin");
  assert.equal(scheduleLabel(zoned), "cron 0 9 * * 1 (Europe/Berlin)");
});

test("the form writes the bare string unless there is a timezone to carry", () => {
  assert.deepEqual(parseSchedule("0 7 * * 1", ""), { cron: "0 7 * * 1" });
  assert.deepEqual(parseSchedule("cron 0 9 * * 1", " Europe/Berlin "), {
    cron: { expr: "0 9 * * 1", timezone: "Europe/Berlin" },
  });
});

test("a timezone with an interval, or with no schedule at all, is refused", () => {
  assert.throws(() => parseSchedule("every 5m", "Europe/Berlin"), /no timezone changes it/);
  assert.throws(() => parseSchedule("", "Europe/Berlin"), /needs a cron schedule/);
  assert.deepEqual(parseSchedule("every 5m", ""), { every: { seconds: 300 } });
  assert.equal(parseSchedule("", ""), null);
});

test("round trip: what the form reads back is what it would send", () => {
  for (const s of [{ cron: "*/5 * * * *" }, { cron: { expr: "0 9 * * 1-5", timezone: "America/New_York" } }]) {
    assert.deepEqual(parseSchedule(scheduleText(s), scheduleZone(s)), s);
  }
});
