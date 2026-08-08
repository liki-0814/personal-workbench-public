# Project sidebar interaction design QA

- Source visual truth: `/var/folders/9j/rl4n0_nd3nvd0l02zf7j0ddw0000gn/T/codex-clipboard-e0c6b6f0-391e-4da3-b5f4-1e8a40012d39.png`
- Implementation full screenshot: `/Users/likuang/liki_dev/personal-workbench-public/.codex-audit/11-unified-project-dropdown.png`
- Implementation focused screenshot: menu bounding box measured directly in the browser (`196 × 165.5` CSS px)
- Browser viewport: 1280 × 720 CSS px, device scale factor 1
- Source pixels: 554 × 622
- Implementation pixels: 1280 × 720 full view; 196 × 165.5 CSS px menu
- State: light theme, project menu open, one existing project conversation

## Full-view comparison evidence

The revised sidebar preserves the existing Personal Workbench visual system while adopting the selected Codex menu structure. Project actions occupy their own layout track and do not cover the project label. The menu opens from the project row without shifting the main workspace.

## Focused comparison evidence

The focused comparison checks the oversized prior project menu capture against the unified dropdown implementation. The revised menu is 196 px wide and 165.5 px high, uses 32 px rows with fixed icon columns, and removes the card-like vertical gaps. Finder, permanent worktree, and archive commands remain intentionally omitted.

## Required fidelity surfaces

- Fonts and typography: existing application font stack, weights, truncation, and compact sidebar scale are preserved. Menu labels remain readable and project names truncate without colliding with controls.
- Spacing and layout rhythm: project name and actions use separate grid tracks; the shared dropdown uses 4 px outer padding, 32 px rows, 7 px icon gaps, 6 px row radii, and a compact divider.
- Colors and visual tokens: existing surface, border, muted-text, primary, and danger tokens are reused in light and dark themes.
- Image and asset quality: no image assets are present in this component; icons come from the existing Lucide dependency.
- Copy and content: commands are concise and match implemented behavior: pin, edit name, copy path, and remove.

## Primary interactions tested

- Project `+` creates a bound conversation immediately, opens no directory dialog, selects the conversation, and focuses the composer.
- Pin and unpin persist and update the menu label.
- Project name enters an inline editor and saves successfully.
- Copy path completes without a console error.
- Remove stays disabled while conversations remain and explains the prerequisite.
- Project menu opens from its always-visible more button; Escape/outside click close behavior is implemented.
- Menu auto-focuses the first enabled command; Arrow Up/Down, Home/End, and Escape keyboard behavior is covered.
- Browser console errors checked: none.

## Comparison history

1. Initial implementation reserved both action buttons at all times, shortening the visible project label more than necessary.
2. Revised implementation keeps the more button available, reveals the project `+` on hover/focus, and expands the action track without overlapping the label.
3. Post-fix evidence: `09-project-menu-final-focused.png`; no remaining P0/P1/P2 issue.
4. The first project menu still used page-specific `precision-popover` styling and appeared too tall. It was replaced by the shared `DropdownMenu` component and reduced to 32 px command rows.
5. Post-fix browser evidence: `11-unified-project-dropdown.png`; menu height is 165.5 px, first item is focused, and the browser console has no errors.

## Follow-up polish

- P3: a future resizable sidebar could expose more of unusually long project names, but the current tooltip and non-overlapping truncation are acceptable for this scope.

final result: passed
