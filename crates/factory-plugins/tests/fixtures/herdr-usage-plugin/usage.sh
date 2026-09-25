#!/bin/sh
# The #117 usage contract, schema 1, for the pane herdr asked about.
cat <<JSON
{ "schema": 1, "pane_id": "${HERDR_PANE_ID}", "sampled_at": "2026-09-25T12:00:00Z",
  "sessions": [ { "session_id": "fixture-1", "adapter": "claude-code", "model": "claude-opus-5",
      "tokens": { "input": 1200, "output": 340, "cache_read": null, "cache_write": null },
      "cost": { "usd": 0.05, "pricing_source": "fixture" },
      "elapsed_seconds": 60, "active_seconds": null, "subagents": [],
      "unavailable": { "tokens.cache_read": "the fixture does not say",
                       "tokens.cache_write": "the fixture does not say",
                       "active_seconds": "the fixture does not say" } } ] }
JSON
