# Model settings design QA

- Source visual truth: `/var/folders/9j/rl4n0_nd3nvd0l02zf7j0ddw0000gn/T/codex-clipboard-ed9dcc29-8f82-417a-98d6-a16f6340fa0c.png`
- Implementation screenshot: `/private/tmp/pwcli-model-settings-implementation.png`
- Source pixels: 2608 × 1690.
- Implementation pixels: 1280 × 720.
- CSS viewport: 1280 × 720; reported device pixel ratio: 2. The browser screenshot API returned a normalized 1280 × 720 image.
- State: light theme, Settings → AI 模型, all providers visible, Grok/xAI expanded.

## Evidence

The implementation was opened from the installed `pwcli` daemon in the in-app browser. The following primary interactions were exercised:

- Open Settings.
- Switch from AI Provider to AI 模型.
- Inspect provider filtering, expand/collapse controls, model visibility toggles, default-model controls, and add-model inputs.
- Open 添加 AI 服务, select Google Antigravity from the native login-service dropdown, and verify that 继续 becomes enabled.
- Verify that built-in catalog models have no delete action (`删除 grok-4.5` locator count: 0).
- Check browser console errors (none).

The implementation preserves the reference's main information architecture: provider navigation on the left, visibility counts, provider-grouped expandable model rows, per-model visibility controls, and explicit default-model selection. It intentionally uses the existing Personal Workbench settings shell, typography, purple accent, and component tokens instead of cloning the reference application's chrome.

## Required fidelity surfaces

- Fonts and typography: existing Personal Workbench system-font hierarchy is internally consistent; model IDs use monospace and secondary metadata is visually subordinate.
- Spacing and layout rhythm: the 1120 px model view provides a stable two-column layout inside the existing settings modal, with scroll containment at the 720 px test viewport.
- Colors and visual tokens: existing neutral surfaces, purple selection state, and semantic capability badges remain consistent with the product.
- Image quality and assets: the reference and implementation contain no product imagery requiring generation; icons use the existing Lucide dependency.
- Copy and content: the page states that catalog entries are visible by default, hidden entries leave selectors, exact IDs may be added, and a default model is explicit.

## Comparison history

1. Initial implementation exposed a delete action for every built-in catalog model. This was a P2 behavior and affordance mismatch because catalog models should be hidden, not deleted.
2. The implementation was changed to label maintained entries as `目录`, keep visibility toggles, and expose delete only for custom-added entries.
3. Post-fix browser evidence confirmed that the built-in Grok model has no delete action and all seven configured catalog models remain visible by default.

## Findings

No remaining P0, P1, or P2 issue was observed in the independently inspected implementation state. A strict final visual comparison could not be completed because the browser security policy rejected the local side-by-side comparison document. The policy explicitly prohibited retrying through an alternate or indirect browser path.

## Follow-up polish

- P3: consider a dedicated full-window settings route if model catalogs grow enough that modal scrolling becomes cumbersome.
- P3: add catalog freshness/source metadata when remote catalog refresh is introduced.

## Final result

final result: blocked

Blocker: the required same-input side-by-side source/implementation comparison was rejected by the browser URL security policy. The implementation itself, primary interactions, responsive containment, and console state were verified independently.
