---
name: archify
description: Create validated architecture, workflow, sequence, data-flow, and lifecycle diagrams with the original Archify typed renderers and interactive standalone HTML runtime. Use for technical topology and process diagrams; use generate_image outside this scope.
license: MIT
metadata:
  version: "2.12-pwcli"
  based_on: "tt-a1i/Archify 2.12"
---

# Archify

Generate technical diagrams through pwcli's embedded original Archify renderers. Never hand-write SVG or HTML and never invoke Node. The `render_archify` tool produces the canonical interactive HTML, performs the original Schema/layout validation, and returns a document card.

## Routing boundary

Use Archify for:

- `architecture`: system components, services, infrastructure, cloud/security/network boundaries.
- `workflow`: technical processes, approvals, tool calls, runbooks, CI/CD, incident response.
- `sequence`: API call chains, request lifecycles, cache fallback, async traces, return paths.
- `dataflow`: pipelines, ETL/ELT, lineage, warehouse sync, PII or governance boundaries.
- `lifecycle`: state machines, retries, waiting states, status transitions, terminal outcomes.
- Converting Mermaid `flowchart`, `sequenceDiagram`, or `stateDiagram` input into a polished technical diagram.

Do not use Archify for photos, illustrations, posters, marketing graphics, decorative infographics, product renders, UI mockups, or scientific imagery. Those requests continue through `generate_image`.

When intent is ambiguous, use Archify only if the main question concerns components and relationships, ordered technical actions, actor interactions, data movement, or state transitions.

## Required execution loop

1. Inspect repository evidence first when the diagram describes real code. Never invent components, protocols, owners, regions, security boundaries, or relationships.
2. Choose exactly one diagram type.
3. Call `archify_reference` once for that type. Follow its compact required contract and complete validated example; do not guess fields.
4. Build a complete typed JSON `diagram` with `schema_version: 1`, the selected `diagram_type`, `meta.title`, and the type-specific required collections reported by `archify_reference`.
5. Call `render_archify`.
6. Common legacy architecture fields and connection-label overlap are repaired by the tool. If another validation error remains, change only the diagnosed subject and retry at most twice.

`render_archify` is the final delivery call. After it succeeds, stop; never call it again merely to preserve evidence. Do not follow it with `create_document`, `render_document`, `run_command`, Mermaid, Graphviz, or `generate_image`.

## Diagram selection

| User intent | Type | Primary organization |
|---|---|---|
| What exists and how it connects | architecture | left-to-right components and explicit boundaries |
| What happens and where decisions branch | workflow | lanes, phases, main path, short exception branches |
| Who calls whom and in what order | sequence | participants across the top, time downward |
| Where data originates, transforms, stores, and ends | dataflow | source-to-consumer stages |
| Which states exist and how transitions terminate | lifecycle | main, waiting, and terminal bands |

For pasted Mermaid, retain topology and meaning but discard its styling. Map `subgraph` to a boundary/lane, decision diamonds to decision/security nodes, participants to sequence actors, and start/end markers to lifecycle start/terminal states.

## Layout discipline

- Establish one obvious main path.
- First render with `meta.quality_profile: "standard"`. Upgrade to `showcase` only after the standard layout succeeds and the stricter finish is useful.
- Prefer at most 10 primary nodes on the first pass; group secondary detail instead of drawing every dependency.
- Keep side branches short and attach them near their parent.
- Label only non-obvious, cross-boundary, asynchronous, approval, error, or return relationships.
- Put policies, tech-stack detail, and secondary facts in cards rather than adding crossing arrows.
- Never route a relationship through an unrelated opaque node.
- Keep nodes at least 16px apart, cross container boundaries perpendicularly, and do not run a connection along a container border.
- Use `meta.views` for a small guided reading sequence when it materially helps.
- Set `meta.animation: "trace"` only when motion or presentation is explicitly useful.

Finish truthfully: renderer/schema/layout checks are deterministic, but do not claim that rendered pixels were visually inspected unless they actually were.
