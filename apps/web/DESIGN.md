# AgentPlaza console — design rules

One visual language for every screen, the 3D floor's overlays and the agent views.

## Foundations (keep)
- Palette and type come from `src/styles/tokens.css` only: paper `--paper`, sheet `--sheet`, rules `--rule`/`--rule-faint`, ink `--ink`/`--ink-2`/`--ink-3`, the orange `--signal` (the owner's hand: primary actions, the active item, focus), tones `--ok-ink`, `--warn-ink`, `--crit`, `--info-ink` and their `*-bg` tints.
- Square corners everywhere (`border-radius: 0`). No pills, no rounded chips, no soft shadows; the one shadow is the printed `--print` / `--shadow` offset.
- Type: `--mono` (Martian Mono) for titles, labels, figures, ids, buttons and navigation; `--serif` (Source Serif 4) for prose. Uppercase + letter-spacing only for small labels (≤ 12px).
- Spacing from the `--s*` scale. Icons are the lucide set in `src/ui/icons.gen.ts` via `icon(name)` (15px, 1.75 stroke, square caps).

## Shell
- Sidebar: icon + label per screen, grouped by section; no numbers, no dotted leaders. Counts are small mono badges on the right. Active item: `--hover` background, a 3px `--signal` bar on the left, ink text, icon in `--signal`.
- The sidebar resizes (drag its right edge, 200–420px), collapses to a 60px icon rail (labels become tooltips); a button at the left of the top bar switches between the two, `[` toggles it from the keyboard, and the choice and width persist. Under 900px it is an overlay drawer.
- Top bar: sidebar toggle · screen title · health · STOP ALL.

## Pages
- Every screen is one sheet (`.view`): uniform padding (28px), the same max width (`--content-max`), a head row (`.view-head`: h1 left, actions right, a 2px ink rule under it). No empty left gutter column.
- Sections inside a sheet are `card()`s: a mono header row over a body, 1px `--rule` borders.
- Empty states: an icon, one bold line, one faint sentence, an action when there is one (`empty()` in components.ts).
- Long prose stays readable: at most ~72ch per line.

## 3D floor and agent views
- The 3D world keeps its toon art. Everything drawn over it (name labels, the agent card, the legend, buttons) uses the console's language: square `--sheet` boxes, 1px `--ink` border, mono name, the role as a square tag with the role colour as a 3px left stripe — never a rounded pill. Buttons are the console's `button()`.
- The floor canvas sits in a card like any other section (1px `--rule` border, no rounded corners).
- Agent portraits and avatars: square frames (1px `--ink` border, `--sheet` background), no rounded frames.
