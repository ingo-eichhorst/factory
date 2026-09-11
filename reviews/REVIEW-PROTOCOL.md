# Independent state-first review protocol

Coordinator run: `8f83af48-f692-47e6-bec2-cf69f55ac040`.

## Boundaries

- Your registered child scope and working directory are your own workspace. Write only inside that directory. Read Factory source and company architectural inputs only as needed.
- Do not change implementation, instance configuration, other reviewers' files, existing analysis, runtimes, external services or Git worktrees. Do not send real alerts or inject failures into live systems.
- Use the assigned Factory task run: `run_get`, `run_progress`, material `run_decision`, then `run_complete` with artifact references or `run_block`. Do not work on another run concurrently.
- The operational Factory CLI is not installed. If the shared task tool is unavailable, the repository-owned compatibility interface is `/Users/factory/business-factory/scripts/factory_tasks.py`; inspect its `--help` before use. Do not access task SQLite tables directly.

## Phase 1: discover states, not residues

Analysts receive `scenarios.csv` with only stressor IDs and text. **Do not read `docs/residuality/`, old residue/control tables, or another reviewer's results during this phase.** The skeptical reviewer has a separately assigned audit mandate and may read the named prior analysis.

Read all assigned scenarios. Ask for each mechanism:

1. What state exists before the perturbation? What actual dynamics or assumptions are needed?
2. What transitions follow? Branch when different initial conditions or policies lead elsewhere.
3. What feedback sustains a regime after the initial perturbation? Can it really persist without continued external forcing?
4. What is the basin or condition of entry? What changes break the loop or permit exit?
5. What remains usable, and what is lost or merely unknown?
6. What observation would refute the interpretation?

A fixed endpoint is not automatically an attractor. Distinguish attractor hypotheses, externally driven cycles, policy holds, information-starved waits, ordinary completion, transients, unresolved dynamics and absorbing loss. Neither a safety rule nor a desired repair action is a surviving structure by itself.

**Do not choose a residue count or derive residues in phase 1.** Do not assume every scenario has a determinable long-term state. Prefer honest unresolved entries to invented convergence. Static source inspection supports transition claims but is not an observed production attractor. Small executable models are allowed only as isolated analytical programs and must document all stipulated assumptions.

## Deliverables

Write plain English. Use semicolon-delimited UTF-8 CSV with quoted fields where necessary. No secrets or copied real customer content.

1. `states.csv` with columns:
   `state_id;name;kind;conditions;feedback;entry;exit;survives;lost;falsifier;evidence;source_refs`
2. `trajectories.csv` with columns:
   `trajectory_id;stressors;initial_state;sequence;destination_states;conditions;evidence;falsifier`
3. `coverage.csv` with columns:
   `stressor_id;state_ids;analysis_status;reason`
   Include exactly one row for every assigned ID. Use space-separated state IDs. Where unresolved, use `none` and state the missing dynamics rather than force a state.
4. `report.md`: the key trajectories in simple language, competing interpretations, source-supported versus hypothetical findings, gaps and the next discriminating experiments. Make clear no actual Factory attractor was empirically established unless you genuinely have such evidence.

Use your assigned prefix for state and trajectory IDs. Counts are not prescribed. `analysis_status` should be `conditional`, `source-supported`, `toy-supported` or `unresolved`; a status is not a success score. `evidence` and `source_refs` must distinguish source code, design promises, toy assumptions and qualitative speculation. Coverage means an explicit disposition, not a modelled or solved scenario.

Finish phase 1 with the task record and artifacts. Do not start reading peer reports or redefining residue categories until a separate follow-up task explicitly asks you to do so.
