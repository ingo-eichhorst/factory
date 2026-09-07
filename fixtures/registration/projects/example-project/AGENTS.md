# Example Project

This is the project-level scope for Example Project, a child of the Fixture
Co company root. It is part of the Slice 1 registration fixture and is not a
real project.

## Mandate

This scope owns the Example Project workspace: a fictional service with two
agents, each its own entry in this scope's `agents:` list. `example-project`
is `permanent` and does the day-to-day work of the scope.
`example-project-reviewer` is `temporary` and exists only to carry out a
review task that a human or the parent scope sends in — never one this scope
raises against itself, since a scope cannot target its own agents for
verification (design §6, §12.1). The two conventions immediately below are
added by this scope; they say nothing about company-wide policy, because that
is already established one level up, in the company root's `AGENTS.md`, which
is compiled ahead of this file rather than repeated here.

- Keep all source changes under this project's own directory tree. Never edit
  a file outside `projects/example-project/` from this scope.
- `example-project-reviewer` carries no state across tasks: it is created
  when a review task is delivered and torn down once that task reaches a
  terminal state. Do not treat two review tasks as sharing an instance.

A session running here sees the company root's `AGENTS.md` first, then this
one, in that order (design §2.5): the company conventions on secrets,
external side effects, and durable results still apply, and the two rules
above are strictly additional to them, not a substitute.
