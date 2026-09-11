<!-- Generated exclusively by docs/residuality/a5/dynamics.py. -->
# Model-derived terminal and recurrent sets

**No empirically observed Factory attractor is established here.**
These are exact executions of deliberately small, stipulated toy transition rules.
A locally returning set in this grid is not a proof of robustness to different equations, scales or inputs.

| Case | Recurrent set / boundary | Kind | Seed basin | Local perturbations returning | Meaning |
|---|---|---|---:|---:|---|
| D01 | `[["outside-observation-window"]]` | escape-not-attractor | 7 | 0/0 | Queue exceeds observation window; do not turn divergence into a capped fixed point |
| D01 | `[[0]]` | model-attractor | 4 | 1/1 | Pending queue 0 |
| D01 | `[[4]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 4 |
| D01 | `[[5]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 5 |
| D02 | `[[0]]` | model-attractor | 13 | 1/1 | Pending queue 0 |
| D03 | `[["outside-observation-window"]]` | escape-not-attractor | 7 | 0/0 | Queue exceeds observation window; do not turn divergence into a capped fixed point |
| D03 | `[[0]]` | model-attractor | 6 | 1/1 | Pending queue 0 |
| D04 | `[[0]]` | model-attractor | 6 | 1/1 | Pending queue 0 |
| D04 | `[[10]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 10 |
| D04 | `[[11]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 11 |
| D04 | `[[12]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 12 |
| D04 | `[[6]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 6 |
| D04 | `[[7]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 7 |
| D04 | `[[8]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 8 |
| D04 | `[[9]]` | fixed-without-local-attraction | 1 | 0/2 | Pending queue 9 |
| D05 | `[[2]]` | model-attractor | 13 | 2/2 | Pending queue 2 |
| D06 | `[[1, 0, 0, "run"]]` | model-attractor | 3 | 2/2 | Process running after bootstrap |
| D06 | `[[6, 0, 0, "boot"]]` | model-attractor | 4 | 1/1 | Repeated failed starts; backoff may only change the period |
| D07 | `[[1, 0, 0, "run"]]` | model-attractor | 7 | 2/2 | Process running after bootstrap |
| D08 | `[[0, 0, 0, "boot"], [2, 3, 0, "boot"], [1, 2, 0, "boot"], [0, 1, 0, "boot"]]` | model-attractor | 7 | 6/6 | Repeated failed starts; backoff may only change the period |
| D09 | `[[0, 0, 3, "quarantine"]]` | policy-hold | 7 | 1/1 | No automatic restart; host resources can cool |
| D10 | `[[0, -1]]` | model-attractor | 7 | 1/1 | Permanent suppression induced by the assumed overload response |
| D11 | `[[0, 0], [3, 0], [6, 0], [0, 3], [0, 2], [0, 1]]` | model-attractor | 7 | 7/7 | Alarm flood and mute alternate |
| D12 | `[[0, 0]]` | model-attractor | 7 | 1/1 | Alert queue serviceable under the assumed arrival grouping |
| D13 | `[[1, 0]]` | externally-maintained-hold | 7 | 2/2 | Bounded pending incident but no human service |
| D14 | `[[4, true, "accepted"]]` | model-attractor | 5 | 1/1 | Wrong artifact confidently accepted |
| D15 | `[[0, true, "held"]]` | policy-hold | 5 | 0/1 | Wrong artifact retained but not accepted |
| D16 | `[[4, false, "accepted"]]` | model-attractor | 5 | 1/1 | Corrected artifact accepted under a stipulated perfect repair oracle |
| D17 | `[[0, 1], [1, 0]]` | externally-driven-cycle | 4 | 2/2 | Alternating writers overwrite one another |
| D18 | `[[0, 0], [0, 1]]` | clock-cycle-only | 4 | 2/2 | Writer turn still alternates but business value is stable under enforced fencing |
| D19 | `[["unknown", 1, 3]]` | information-starved-hold | 1 | 1/1 | Outcome unknown; no new call is made |
| D20 | `[["damage", 2, 0]]` | absorbing-damage-fact | 1 | 0/0 | At least two effects happened; future service state is not modelled |
| D21 | `[["confirmed", 1, 3]]` | ordinary-completion | 1 | 0/1 | One effect confirmed after delay; not evidence of autonomous attraction |
| D22 | `[["unknown", 1, 3]]` | information-starved-hold | 1 | 1/1 | Outcome unknown; repeated calls are deduplicated but still consume resources |
| D23 | `[["damage", 2, 0]]` | absorbing-damage-fact | 1 | 0/0 | At least two effects happened; future service state is not modelled |
| D24 | `[["damage", 2, 0]]` | absorbing-damage-fact | 1 | 0/0 | At least two effects happened; future service state is not modelled |
| D25 | `[["quarantine", 1, 0]]` | policy-hold | 1 | 0/0 | Restored intent cannot dispatch automatically |
| D26 | `[[0]]` | model-attractor | 13 | 1/1 | Pending queue 0 |

## Scope and exclusions

- 350 scenario texts screened; 55 linked to a toy mechanism; 295 have no toy trajectory in this pass.
- 26 parameter cases across 6 toy mechanisms.
- 164 initial-state runs. Their seed grid is chosen, not a probability distribution.
- Basin counts are grid cardinalities, not likelihoods of incidents or deployment outcomes.
- A source scenario link means the case isolates one mechanism, not that every detail of that scenario was simulated.
- Queue escape is not counted as an attractor. Writer scheduling is explicitly external forcing.
- Policy holds, information starvation, ordinary completion and irreversible damage are separated from model attractors.

## Recurrent-set classifications (case-specific, not unique business states)

- absorbing-damage-fact: 3
- clock-cycle-only: 1
- escape-not-attractor: 2
- externally-driven-cycle: 1
- externally-maintained-hold: 1
- fixed-without-local-attraction: 9
- information-starved-hold: 2
- model-attractor: 15
- ordinary-completion: 1
- policy-hold: 3
