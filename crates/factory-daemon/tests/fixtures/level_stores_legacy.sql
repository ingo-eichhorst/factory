-- Pre-owner migration fixture; original tables and indexes.

CREATE TABLE IF NOT EXISTS workflow_definitions (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS workflows_scope ON workflow_definitions(scope, updated_at);
CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS workflow_runs_definition ON workflow_runs(workflow_id, updated_at);
CREATE INDEX IF NOT EXISTS workflow_runs_active ON workflow_runs(status, updated_at);
CREATE TABLE IF NOT EXISTS recovery_journal (
    id TEXT PRIMARY KEY,
    action_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    phase INTEGER NOT NULL,
    at TEXT NOT NULL,
    data TEXT NOT NULL,
    UNIQUE(action_id, phase)
);
CREATE INDEX IF NOT EXISTS recovery_journal_scope ON recovery_journal(scope, at);
CREATE TABLE IF NOT EXISTS deployment_mirrors (
    id TEXT PRIMARY KEY,
    deployment TEXT NOT NULL,
    scope TEXT NOT NULL,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS deployment_mirrors_deployment ON deployment_mirrors(deployment);


CREATE TABLE IF NOT EXISTS backup_events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    snapshot TEXT,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS backup_events_at ON backup_events(at);


CREATE TABLE IF NOT EXISTS deploy_events (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    deployment TEXT,
    at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS deploy_events_deployment ON deploy_events(deployment);
CREATE TABLE IF NOT EXISTS health_samples (
    environment TEXT NOT NULL,
    check_name TEXT NOT NULL,
    at TEXT NOT NULL,
    ok INTEGER NOT NULL,
    latency_ms INTEGER NOT NULL,
    slow INTEGER NOT NULL DEFAULT 0,
    detail TEXT
);
CREATE INDEX IF NOT EXISTS health_samples_at ON health_samples(at);
CREATE INDEX IF NOT EXISTS health_samples_check ON health_samples(environment, check_name);

CREATE TABLE IF NOT EXISTS infrastructure_expiries (id TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS credential_expiries (id TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS renewal_push_receipts (identity TEXT PRIMARY KEY, attempted_at TEXT NOT NULL, outcome TEXT NOT NULL);
