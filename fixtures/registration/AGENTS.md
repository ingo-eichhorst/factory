# Fixture Co

This is a test fixture, not a real company. It exists under
`fixtures/registration/` so Slice 1's manual registration procedure has a
worked example to validate against: a company-root scope, one registered
child scope, and the `AGENTS.md` files design §2.5 compiles for each.

## Scope

This directory is the company-root scope for Fixture Co. A registered
directory defines one scope, and each scope configures one or more agents.
Humans may target any registered scope; an agent may target a registered
descendant or sibling scope, but never an ancestor or itself.

## Working conventions

- Keep durable results in repository-owned files or task records; terminal
  scrollback is not canonical state.
- Never place secrets in task prompts, configuration, or generated context.
- Do not overwrite harness configuration or generated compatibility files
  unless an explicit Factory ownership marker permits regeneration.
- Confirm with a human before any external side effect — sending mail,
  changing an invoice, touching a third-party system.

Every registered descendant inherits this file ahead of its own, in the order
company context, then ancestor scopes, then the current scope (design §2.5).
The child scope's file, `projects/example-project/AGENTS.md`, adds to these
conventions; it does not repeat or replace them.
