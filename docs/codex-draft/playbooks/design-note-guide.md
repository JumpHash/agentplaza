# Design note guide
How to write a design note that lets a planner split the work and lets builders work in parallel without asking questions. Architects use it; planners, builders and reviewers read design notes against it.

## When to use
- A new spec or a spec change needs boundaries, interfaces or data models decided before planning.
- A change touches an interface other code, repos or projects depend on.
- Builders or reviewers keep asking the same design question; answer it by updating the design note, not in side conversations.

## Steps
1. Read the spec and the existing code until you can explain how the system works today. You can read repos through Weft and earlier design notes with `read_artifact`.
2. Write the context: what the spec asks for, what exists, what must not change.
3. List the forces: requirements, constraints, risks. Include platform constraints that shape the design: agent VMs have no NIC and reach the outside only through the egress proxy, every tool call goes through the gateway, which checks the JHRL laws, and irreversible actions need the owner.
4. Sketch at least two real options. For each, write honest `pros` and `cons`. A design note with one option fails the schema's intent and the role's rule.
5. Choose one. Prefer the simplest design that meets the spec; justify every added component. If two options are close, pick the one that is easier to change later.
6. Define interfaces precisely: name, inputs, outputs and errors. Two builders should be able to implement both sides in parallel from the contract alone.
7. Define data models: fields, types, ownership, and what is persisted where.
8. List `constraints_respected`: the laws, decisions and limits the design relies on, each named (for example "agent VMs have no NIC").
9. List `risks` and how the design limits each.
10. Write `guidance_for_planner`: how to split the work, what to build first, which parts can run in parallel, and where acceptance tests should focus.
11. Submit with `submit_design` using the **design-note** schema: `spec_ref`, `context`, `decision`, `options`, `interfaces`, `data_models`, `constraints_respected`, `risks`, `guidance_for_planner`.

## Checklist
- [ ] `spec_ref` points at the exact artifact or commit the design is based on.
- [ ] At least two options are compared, each with pros and cons.
- [ ] The decision says which option was chosen and why.
- [ ] Every interface has a name and a contract with inputs, outputs and errors.
- [ ] Data models name their fields, types and owner.
- [ ] Platform constraints the design depends on are listed by name.
- [ ] Nothing in the design needs a law, the gateway or the sandbox to be bypassed.
- [ ] Any irreversible or external-facing step is marked as needing owner approval.
- [ ] Every added component is justified by a requirement.
- [ ] Risks are listed with their mitigations.
- [ ] Planner guidance gives an order of work and points of parallelism.
- [ ] Public interfaces used by other projects are flagged for escalation.

## Pitfalls
- A one-option design hides the tradeoff from the planner and reviewers, so always write down the real alternative you rejected.
- Vague interfaces such as "a service handles it" force builders to guess and collide, so name the call, its fields and its errors.
- Designing around a law, for example assuming direct network access from a VM, produces work that cannot run, so if the spec needs something forbidden, say so and escalate.
- Gold-plating beyond the spec costs budget at every later step, so drop components no requirement needs.
- Answering design questions in messages leaves the note stale, so update the design note and point to it.
- Copying text from a research brief into the design carries untrusted content forward, so summarise the facts and cite the brief as a source.
- Changing a public interface other projects rely on without escalation breaks them silently, so escalate before submitting.
- Writing an essay instead of a note wastes frontier-rung tokens for every reader, so keep it short and precise.
