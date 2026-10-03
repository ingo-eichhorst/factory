# Release artifact provenance

Factory emits local, unsigned in-toto Statement v1 records with a SLSA
Provenance v1 predicate. This is an evidence format, not a certification of
SLSA isolation or signing. The daemon, its filesystem and database, the
executing agent and the instance owner share a trust boundary. Roles limit
accidental actions; they are not an adversarial security boundary.

## Produce and retrieve

Build outputs in the run's Git working directory, then explicitly report them:

```sh
factory task report --status done --result "built release" \
  --artifact dist/release.tar --artifact dist/sbom.cdx.json
factory run provenance <run-id>
factory run provenance <run-id> --json
```

`--artifact` is repeatable and valid only with `--status done`. Paths are
resolved by the daemon relative to that attempt's working directory, not the
CLI's current directory. HTTP callers use `POST /api/tasks/{id}/report` with
`artifacts: ["dist/release.tar"]` in the ordinary done report. The callback
token and authorization are exactly the report's existing contract.
`GET /api/runs/{id}/provenance` returns the same `run_provenance` payload as
the CLI. Its `records` array contains artifact metadata and a `statement` per
artifact. Use that `statement` when exporting to in-toto tools.

The daemon captures actual bytes before beginning verification. Files must be
regular, at most 256 MiB each, with at most 16 unique filenames per report.
Canonical paths must stay within the run's working directory; Git internals,
Factory state, directories and escaping symlinks are refused. Git must have a
readable commit. Runs that report no artifacts retain their previous behavior,
including non-Git projects and legacy data.

Source and artifact bytes must stay unchanged through verification. Gates
that modify source or outputs require rebuilding and reporting artifacts
again. No artifact provenance is exposed while the run is verifying,
blocked, failed or cancelled. Matching gates and independent reviews must
pass the frozen required steps before publication; person approvals preserve
their pre-dispatch source evidence because they authorize the build, not its
later output. Missing, rejected, self-issued or wrong-functionary evidence
cannot satisfy a required step. Failed publication leaves verification
blocked with a journal reason and a live callback token for repair.

The snapshot resides under the instance's
`.factory/artifacts/<run-id>/<artifact-id>/<filename>`. Capture never
overwrites a previous file. Records are append-only in the existing policy
SQLite store, alongside run attestations; identical inserts are idempotent,
and conflicting evidence for the same id is refused. Only records matching
the completed run's captured artifacts and finish time are returned. Failed
captures can leave unreferenced instance-owned snapshots; they are not
published evidence. These are runtime artifacts, not authored scope content,
and have no automatic retention/deletion or separate backup promise.

## Build type v1

Build type URI:
`https://github.com/ingo-eichhorst/factory/blob/main/docs/artifact-provenance.md#build-type-v1`.

This template means: dispatch the identified Factory task, execute its
instructions in the run's working directory, explicitly hand in the named
outputs with a done report, and satisfy the run's frozen control plan. It
describes one Factory attempt, not every network input or a reproducible,
hermetic toolchain. To initiate it, inspect `factory task show <taskId>` and
run `factory task run <taskId>` in the identified instance/scope. Task
instructions may change later: the task id is a reference, not an immutable
copy of all build inputs. Rebuilding from this record alone is not promised.

`externalParameters` has exactly `taskId`, `scope`, and `category` strings.
These are caller-controlled input references; consumers must check that they
identify the expected release workflow. No internal parameters are emitted.
`resolvedDependencies` describes the source with `gitCommit` and
`factoryWorktreeSha256` digests. The latter uses the verifier's existing
`factory-worktree-v1` algorithm: HEAD, porcelain status, binary tracked diff,
and untracked names/bytes. Ignored files are not source dependencies, but an
explicitly selected ignored output is independently hashed and validated.
This is not an inventory of downloaded dependencies; build SBOM attachments
remain separate.

The subject's `sha256` is the SHA-256 of the captured output bytes.
`runDetails.builder.id` identifies the local Factory instance;
`metadata.invocationId`, `startedOn`, and `finishedOn` identify the attempt
and match its times. The URI-named extension
`https://github.com/ingo-eichhorst/factory/run-evidence/v1` carries the frozen
worker, adapter, runtime, source snapshot, required functionaries and step
attestations. Requirement references are opaque: L4 does not evaluate a
framework or control name. Callback tokens are not serialized into provenance.
Gate output and review findings are existing audit evidence: avoid putting
credentials into them.

A complete minimal exported statement (a run with no required steps):

```json
{
  "_type": "https://in-toto.io/Statement/v1",
  "subject": [{"name": "release.tar", "digest": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}],
  "predicateType": "https://slsa.dev/provenance/v1",
  "predicate": {
    "buildDefinition": {
      "buildType": "https://github.com/ingo-eichhorst/factory/blob/main/docs/artifact-provenance.md#build-type-v1",
      "externalParameters": {"taskId": "t1", "scope": "demo", "category": "release"},
      "resolvedDependencies": [{"name": "source", "digest": {"gitCommit": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "factoryWorktreeSha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}}]
    },
    "runDetails": {
      "builder": {"id": "urn:factory:instance:local"},
      "metadata": {"invocationId": "r1", "startedOn": "2026-10-01T09:00:00Z", "finishedOn": "2026-10-01T09:05:00Z"}
    },
    "https://github.com/ingo-eichhorst/factory/run-evidence/v1": {
      "agent": "shell", "adapter": "shell", "runtime": "herdr",
      "source": {"commit": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "worktree_digest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc", "dirty": false},
      "required_steps": [], "attestations": []
    }
  }
}
```

Format references: [SLSA build provenance](https://slsa.dev/spec/v1.2/build-provenance)
and [in-toto Statement v1](https://github.com/in-toto/attestation/blob/main/spec/v1/statement.md).
