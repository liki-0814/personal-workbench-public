# Archify JSON IR Schemas

Each typed renderer consumes a JSON intermediate representation (IR) validated
against one of the schemas in this folder before any layout work happens.

## Files

| Schema | Governs | Structural arrays |
|--------|---------|-------------------|
| `workflow.schema.json` | `diagram_type: "workflow"` | `lanes`, `phases`, `groups`, `mainPath`, `nodes`, `edges` |
| `sequence.schema.json` | `diagram_type: "sequence"` | `participants`, `segments`, `messages`, `activations` |
| `dataflow.schema.json` | `diagram_type: "dataflow"` | `stages`, `nodes`, `flows` |
| `lifecycle.schema.json` | `diagram_type: "lifecycle"` | `lanes`, `states`, `transitions` |
| `architecture.schema.json` | `diagram_type: "architecture"` | `components`, `boundaries`, `connections` |
| `common.schema.json` | shared `$defs` only (no top-level document) | — |

Every diagram schema requires `schema_version`, `diagram_type`, and `meta`
(with `title`). Required structural collections vary by diagram type:

- Architecture requires `components`; `layout`, `boundaries`, `connections`,
  and `cards` are optional.
- Workflow requires `lanes`, `nodes`, and `edges`; `phases`, `groups`,
  `mainPath`, and `cards` are optional.
- Sequence requires `participants` and `messages`; `segments`, `activations`,
  and `cards` are optional.
- Data flow requires `stages`, `nodes`, and `flows`; `cards` is optional.
- Lifecycle requires `lanes`, `states`, and `transitions`; `cards` is optional.

Schemas set `additionalProperties: false` at object levels, so unknown fields
are rejected rather than silently ignored.

Every `meta` object also accepts `animation: "trace"` for opt-in SVG/CSS motion
in generated HTML. Omit it, or set `"none"`, for the default static output.
`visual_preset` accepts `classic` (the stable default), `signal-flow` (luminous
motion-forward presentation), `blueprint` (high-contrast engineering review),
or `editorial` (warm publication-style design review and documentation).
Presets change only viewer styling; they do not alter semantic IDs or geometry.
It may also include up to five guided `views`. Each view has a unique `id`, a
reader-facing `label`, a non-empty `focus` list of existing semantic node IDs,
and an optional short `note`.

Every relationship collection (`connections`, `edges`, `messages`, `flows`, and
`transitions`) accepts an optional author-controlled `id` using the shared ID
pattern. The renderer keeps its source-order runtime key separately, while the
authored ID enables a stable `#relation=<id>` viewer link that survives array
reordering. ID-less documents remain valid and their relationship pins stay
local to the current page.

## schema_version policy

`schema_version` is `"const": 1`. The constant pins the IR contract: a file
that validates today keeps validating and rendering on every 2.x release.
Additive viewer, accessibility, and presentation improvements may enhance the
generated HTML, but they must not reinterpret authored IR or turn a previously
valid profile-less v1 file into a new hard layout failure. A breaking change to
any IR shape bumps the constant to `2`; renderers will then reject version-1
files with a clear schema error instead of misrendering them. Additive,
backwards-compatible fields do not bump the version.

## Shared definitions (common.schema.json)

The five diagram schemas reference `common.schema.json#/$defs/...`:

- `id` — element identifiers, pattern `^[a-zA-Z][a-zA-Z0-9_-]*$`
- `point` — an `[x, y]` pair of numbers (used by `via` and `labelAt`)
- `componentType` — `frontend`, `backend`, `database`, `cloud`, `security`,
  `messagebus`, `external`
- `variant` — `default`, `emphasis`, `security`, `dashed` (sequence messages
  extend this list locally with `return`)
- `guidedViews` — the bounded, read-only reader paths accepted by `meta.views`
- `cards` — the summary-card blocks rendered below the SVG

Lifecycle state `type` is mode-specific (`start`/`active`/`waiting`/...) and
stays in `lifecycle.schema.json`.

## Runtime validation

The committed `renderers/shared/generated-validators.mjs` contains standalone
validators for all five schemas, so renderer runtime validation has no npm or
network dependency. `renderers/shared/validator.mjs` applies the matching
validator before the renderer's own layout checks.
The shared loader then checks cross-collection facts that JSON Schema cannot
express cleanly here: duplicate view IDs, duplicate focus IDs, focus IDs that do
not exist in the diagram's semantic collection, and duplicate authored
relationship IDs within the mode's relationship collection.

Architecture additionally supports opt-in, revision-pinned repository evidence.
`meta.repository` names a public GitHub URL and full commit SHA; a component may
carry one to three `sources` with repo-relative POSIX paths, optional line
ranges, and optional labels. Shape is schema-checked, then the Node CLI requires
the `ARCHIFY_REPO_ROOT` environment variable to name the local checkout. Its Git
origin must match, and Git must prove the commit, blobs, and requested lines.
Verified evidence is embedded outside the canonical SVG for the Semantic
Passport and Node Finder; ordinary documents and visual exports carry no
repository evidence.

## Visual quality and engineering truth

`meta.quality_profile` and `meta.engineering_profile` answer different
questions. `quality_profile` is available in all five modes and controls how
strictly Archify judges composition. `engineering_profile` is an optional
Architecture-only semantic contract; omitting it preserves the ordinary v1
behavior.

The first engineering profile is `deployment-ownership`. Enable it only when
the user wants a fail-closed deployment review and the source facts are known.
It requires every non-external component to name an owner in `tag` and belong
to exactly one `region`; the document must contain both `region` and
`security-group` boundaries; every `database` must be inside a
`security-group`; each security group must contain members from one shared
region; and every connection whose region or security-group membership changes
must name the real crossing mechanism in `label`.

The profile validates only authored IR. It does not discover infrastructure,
infer owners, or prove that a diagram matches a live environment. If a fact is
unknown, leave the profile unset or obtain the fact instead of inventing it.

`npm run build:archify` bundles the five embedded runtimes and verifies that
each embedded renderer produces the same HTML as its Node CLI for the bundled
comparison example. `cargo test` covers the Rust-side Archify tool contract and
renders all five embedded examples.

## Error format

Schema violations exit non-zero. Each ajv error is reported on its own line as
the instance path — annotated with the nearest enclosing element's `id` or
`label` — followed by the message and parameters:

```text
workflow schema validation failed:
  /nodes/3 (id/label: "router") must NOT have additional properties {"additionalProperty":"colour"}
```

Schemas catch shape errors (types, enums, ranges, unknown fields); geometry
problems such as overlaps and label collisions are the renderers' job.
