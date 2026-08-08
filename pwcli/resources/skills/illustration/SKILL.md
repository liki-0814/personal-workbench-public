---
name: illustration
description: Route scientific diagrams, data-faithful plots, image polishing/refinement, and illustration evaluation through pwcli's deterministic illustration workflow.
---

# Scientific illustration

Use `illustrate` for publication-oriented scientific figures:

- Method or concept illustration → `mode=diagram`.
- Structured raw data → `mode=plot`; never use an image model for evidence-bearing data.
- Existing scientific figure with a targeted edit → `mode=refine`.
- Existing figure needing global visual cleanup while preserving facts and values → `mode=polish`.
- Figure quality/faithfulness assessment → `mode=eval`.

Routing boundaries:

- Infrastructure topology, API sequence, state machine, CI/CD workflow, or data lineage → use the `archify` skill instead.
- General photos, marketing art, standalone illustration assets, or product imagery → use `generate_image` instead.

Pass original facts in `content`, the desired communication goal in `visualIntent`, and every non-negotiable fact in `constraints`. Keep `retrieval=auto` unless the user explicitly opts out. Use `quality=balanced` by default; use `max` only when the user values quality over latency and model cost.

The built-in starter references teach layout and style only. Never treat their labels or values as facts for the user's figure.
