# A6 independent-agent review process

## What the user requested

Use subagents for the attractor-first analysis rather than relying on the same analyst to propose and confirm the states and residues.

## Separation before synthesis

Four analysts receive disjoint scenario-text inputs covering the full existing corpus:

| Reviewer | Input | Discovery boundary |
|---|---|---|
| Operations | S001–S100 | Physical failure, storage, runtime, effects and initial operator cases |
| Governance/change | S101–S200 | Models, security, organization, legal obligations, evolution and extremes |
| Observation/timing | S201–S275 | External observation, latency, queues, control and trust |
| Meaning/boundaries | S276–S350 | Evidence, compatibility, people, providers, law, safety and compound extremes |

The analyst-facing inputs contain **only ID and scenario text**. Prior residue/control assignments and proposed surviving capabilities are removed. Input hashes are recorded in [input-manifest.json](input-manifest.json). Analysts are instructed not to read the prior residuality package or one another's outputs during initial discovery. They may inspect actual implementation and architectural source documents read-only.

A fifth reviewer independently audits A5's method, transition rules, basin tests and residue derivation. That reviewer is intentionally not blinded to A5. A subsequent task asks for adversarial cross-review only after the four original reports are complete.

This is **context separation, not proof of independent expertise**. The agents use the same harness and may share the same model family and training biases. The scenario texts themselves were selected and worded by the earlier analyst. Agreement is not an empirical guarantee; disagreements and falsifiers must remain visible.

## Required discovery artifacts

Each analyst produces:

- `states.csv`: conditions, sustaining feedback, entry, exit, surviving and lost capabilities, falsifier and evidence.
- `trajectories.csv`: scenario → starting state → conditional transitions → candidate destinations.
- `coverage.csv`: one explicit disposition for every assigned scenario, including unresolved dynamics.
- `report.md`: plain-language findings and limits.

No residue count and no phase-1 residue proposal is requested. A state name can be a hypothesis, policy hold, missing-information wait, forced cycle or loss state; it is not automatically an attractor. Reference integrity is checked separately from the truth of a causal claim.

## Synthesis rules

1. Preserve the original reviewer artifacts and hashes before cross-review.
2. Do not sum the reviewers' label counts and present the sum as globally distinct attractors.
3. Keep a state separate if the sustaining mechanism, observability, authority, retained work or exit condition differs materially.
4. Merge only with a reason that survives counterexamples, not because labels sound alike.
5. Keep dissent or uncertain classification explicit. An imposed stop, lost key or absent human is not automatically autonomous attraction.
6. Derive candidate surviving structures only after discussing the state findings and skeptical objections. Do not force the result to 17, 14 or any larger target.
7. Do not promote conditional or source-supported claims to observed production attractors. Source inspection can support a transition but not establish a long-run basin by itself.

## Operational record

The coordinator is Factory run `8f83af48-f692-47e6-bec2-cf69f55ac040`. [review-manifest.json](review-manifest.json) records delegated tasks, runs, agents, workspaces and exact returned pane IDs.

The instance permits delegation to registered descendant or sibling scopes, not back to this same scope. Accordingly, five new child review scopes were registered centrally with separate directories, local `AGENTS.md` mandates and `temporary` agent lifetimes. No `.factory/` directory or runtime database was placed inside a review scope.

All five workspaces were created with `--no-focus`. Interactive shell state was checked before startup, and name/cwd/readiness were verified afterwards. Four initial names exceeded Herdr's 32-character limit and were rejected before startup; only those new role suffixes were shortened. No unrelated pane or agent was interrupted or repurposed.

Ordinary work was delivered through assigned durable Factory runs, using the existing compatibility `run-deliver` path because the production `factory` CLI is not installed. No direct ad hoc prompt replaced a durable task.

No live failure injection, real alert, external provider mutation, product implementation change, Git worktree operation or publishing is authorized by this analysis. The review's scripts and reports are agent work products, not Factory kernel file writes.
