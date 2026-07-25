# Talking Quill — Visual Redesign Style Guide (v2)

This document is the single source of truth for the 2024 redesign. **Only visual
presentation changes. No feature, behaviour, copy, naming, IPC, or a11y semantics
may change.** The product is called **Talking Quill** (never "Talking Pen").

---

## 1. Design intent

Editorial, quiet, precise. Think a well-set book page, not a dashboard.

- **No cards.** Sections are flat: a small heading, then a grouped list of rows.
- **Small type.** Base UI size is 13px. Nothing in the chrome is larger than 20px
  except the brand wordmark.
- **Tight rhythm.** 4px grid. Paddings are 6–12px, not 24–40px.
- **Hairlines, not boxes.** Grouping is done with a 1px border around a *group of
  rows* plus 1px inner dividers — never with a shadowed card per control.
- **One accent.** Used for the active nav marker, focus ring, toggles on, primary
  button. Everything else is neutral ink.
- **Both themes are first-class.** Light = warm paper. Dark = deep ink navy.

Reference image: `C:\Users\user\Downloads\talkingquill\design.png`.

---

## 2. Tokens

All tokens live in `app/src/renderer/design/tokens.css`. **Never hardcode a
colour, radius, font or spacing value in a component or screen stylesheet — use a
token.** If you need a new value, add a token.

### Spacing (4px grid)

| Token | Value | Use |
|---|---|---|
| `--space-1` | 4px | icon gaps, inline gaps |
| `--space-2` | 8px | row gaps, control padding |
| `--space-3` | 12px | row inline padding, section body gap |
| `--space-4` | 16px | gap between sections |
| `--space-5` | 24px | screen padding |
| `--space-6` | 32px | rare, large screen padding |

### Radii

`--radius-sm: 6px`, `--radius-md: 8px`, `--radius-lg: 10px`, `--radius-pill: 999px`.

### Typography

- `--font-ui`: `'Inter var','Inter',-apple-system,'Segoe UI Variable Text','Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif`
- `--font-display`: `'Iowan Old Style','Palatino Linotype',Palatino,Georgia,Cambria,'Times New Roman',serif`
- `--font-mono`: `ui-monospace,'Cascadia Mono','SF Mono',Consolas,monospace`

CSP is `font-src 'self'` — **do not add webfonts or `@import` from a CDN.**

| Token | Size / weight | Use |
|---|---|---|
| `--fs-2xs` | 10px | badge text, progress caption |
| `--fs-xs` | 11px | eyebrow, meta, hints |
| `--fs-sm` | 12px | secondary text, hints, table meta |
| `--fs-md` | 13px | **body / all controls (default)** |
| `--fs-lg` | 15px | section heading (`h2`) |
| `--fs-xl` | 19px | screen title (`h1`) |
| `--fs-brand` | 22px | brand wordmark (display serif) |

Weights: 400 body, 500 emphasis, 600 headings/labels. **Never use 700+ except the
brand wordmark.** Never use `letter-spacing` except the eyebrow (`0.09em`) and the
brand tagline.

### Colour — semantic names only

```
--color-background   page canvas
--color-surface      chrome (titlebar, sidebar)
--color-raised       grouped-row panel / hover
--color-row          a single row inside a group
--color-border       hairline
--color-border-strong  hairline for hover / dashed empty state
--color-text         primary ink
--color-secondary    supporting copy
--color-muted        disabled / placeholder
--color-accent       interactive accent
--color-accent-soft  accent tint background
--color-brand        brand wordmark ink (gold in dark, navy in light)
--color-success / --color-warning / --color-error
--color-success-soft / --color-warning-soft / --color-error-soft
--shadow-1           subtle popover shadow
--focus-ring
--transition-fast    120ms cubic-bezier(.2,.6,.2,1)
```

Themes are selected with `:root[data-theme='light']` / `:root[data-theme='dark']`.
`color-scheme` must be set per theme so native scrollbars/controls follow.

---

## 3. Core layout

```
┌──────────────────────────────────────────────┐
│ brandbar  [logo] Talking Quill   ☾ ─ □ ×     │  56px, draggable
│           Speak naturally. Write effortlessly.│
├────────────┬─────────────────────────────────┤
│ sidebar    │ content                          │
│ 188px      │                                  │
│ nav items  │  screen                          │
│            │                                  │
│ status     │                                  │
└────────────┴─────────────────────────────────┘
```

- Brand bar: `--color-surface`, bottom hairline, `-webkit-app-region: drag`
  on the brand block only. Logo 30px, `border-radius: 50%`.
- Wordmark: `--font-display`, `--fs-brand`, weight 600, `--color-brand`.
- Tagline: `--fs-xs`, `--color-secondary`, letter-spacing `0.02em`.
- Window controls: 40×32 quiet buttons, radius 0, close hover = error.
- Theme toggle sits immediately left of the window controls; it is a quiet 28×28
  icon button with `aria-label` "Switch to dark theme" / "Switch to light theme".

---

## 4. Component recipes

### 4.1 Section (replaces Card)

`Card` keeps its **exact public API** (`title`, `description`, `interactive`,
`disabled`) so no call site changes, but renders flat:

```html
<div class="me-card">
  <header class="me-card__header">
    <h2 class="me-card__heading">Keybindings</h2>
    <p class="me-card__description">…</p>
  </header>
  <div class="me-card__body"> …children… </div>
</div>
```

- `.me-card` — `display:grid; gap:var(--space-2)`. **No border, no background,
  no padding, no shadow.**
- `.me-card__heading` — `--fs-lg`, weight 600.
- `.me-card__description` — `--fs-sm`, `--color-secondary`.
- `.me-card__body` — `display:grid; gap:var(--space-2)`.

### 4.2 Row group (`.group`) — the picture's core motif

Use for lists of settings, readiness lines, key/value details, history entries.

```css
.group { border:1px solid var(--color-border); border-radius:var(--radius-md);
         background:var(--color-raised); overflow:hidden; }
.group > * + * { border-top:1px solid var(--color-border); }
.row { display:flex; align-items:center; gap:var(--space-3);
       min-height:34px; padding:var(--space-2) var(--space-3); }
.row__label { flex:1; min-width:0; }
```

Inside `.me-card__body`, these element types are automatically row-styled and
grouped (see `components.css`): `.me-toggle`, `.readiness-row`, `.setting-action`,
`.details-list > div`, `.settings-list > li`.

### 4.3 Controls

| Control | Height | Notes |
|---|---|---|
| `.me-button` | 28px | radius `--radius-sm`, weight 500, `--fs-md`, padding `0 10px` |
| `.me-button--quiet` | 26px | transparent, `--color-secondary` |
| `.me-field__control` (input/select) | 28px | padding `0 8px`, radius `--radius-sm`, bg `--color-surface` |
| `.me-toggle__control` | 30×17 | knob 13px, radius pill |
| `.me-status` | 20px | pill, `--fs-xs`, weight 500, tinted `*-soft` bg, no border |
| `.me-progress__bar` | 4px | radius pill |

Buttons are **never** full-width unless the parent is a narrow form column.
Primary button = accent bg, `--color-on-accent` text. Secondary = `--color-surface`
+ hairline. Danger = error text on transparent + error hairline (not a solid red
block).

Labels above fields: `--fs-xs`, weight 500, `--color-secondary`, gap 4px.

### 4.4 Nav item

```css
.nav-item { position:relative; display:flex; align-items:center; gap:var(--space-2);
  min-height:30px; padding:0 var(--space-2) 0 var(--space-3);
  border:0; border-radius:var(--radius-sm); background:transparent;
  color:var(--color-secondary); font-size:var(--fs-md); }
.nav-item[aria-current='page'] { background:var(--color-raised); color:var(--color-text); font-weight:500; }
.nav-item[aria-current='page']::before { content:''; position:absolute; left:0; top:6px; bottom:6px;
  width:2px; border-radius:1px; background:var(--color-accent); }
```

Icon slot is a 14px inline SVG (`currentColor`, `stroke-width:1.5`), `opacity:.75`.

### 4.5 Empty state

Dashed hairline, radius md, padding `var(--space-5) var(--space-4)`, title
`--fs-md` weight 600, description `--fs-sm` secondary. No icon circle, no accent
fill.

### 4.6 Dialog / popover

`--color-surface` bg, hairline, `--radius-lg`, `--shadow-1`, padding
`var(--space-4)`. Title `--fs-lg`. Backdrop `rgb(0 0 0 / 40%)` + `backdrop-filter: blur(2px)`.

---

## 5. Icons

No emoji, no `◉ ⚙ ⓘ ☾` glyphs. Use inline SVG from
`app/src/renderer/design/Icon.tsx`:

```tsx
<Icon name="dashboard" />   // 14px by default, size prop for 16/18
```

Available names must include: `dashboard`, `settings`, `info`, `general`, `profiles`,
`recording`, `model`, `privacy`, `smart`, `commands`, `vocabulary`, `sun`, `moon`,
`minimize`, `maximize`, `restore`, `close`, `chevron`, `check`, `search`, `trash`,
`copy`, `plus`. All 16×16 viewBox, `fill="none" stroke="currentColor"
stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"`.

Icons are decorative: `aria-hidden="true"` and `focusable="false"`.

---

## 6. Theming

`app/src/renderer/design/theme.ts` exports:

```ts
export type ThemeName = 'light' | 'dark';
export function readStoredTheme(): ThemeName | null;
export function resolveInitialTheme(): ThemeName;   // stored ?? prefers-color-scheme
export function applyTheme(theme: ThemeName): void;  // sets <html data-theme>
export function useTheme(): [ThemeName, (next: ThemeName) => void];
```

- Storage key: `talking-quill.theme` in `localStorage`, wrapped in try/catch.
- `applyTheme` also writes `document.documentElement.dataset.theme`.
- Widget renderer calls `applyTheme(resolveInitialTheme())` on mount and listens
  to `window.addEventListener('storage', …)` so both windows stay in sync.
- Never touch settings schemas / IPC for the theme.

Logo assets: `app/assets/logo-light.png` (use in **light** theme) and
`app/assets/logo-dark.png` (use in **dark** theme). Import both and pick by theme.

---

## 7. Settings navigation (tabs)

`SettingsScreen` keeps: the `sections` array, its titles, its keywords, the
search box, the `normalizeSearch` helper, the save/notice logic, and the
`role="region"` + `aria-label={title}` wrapper. It changes only in **layout**:

- A vertical tab rail on the left (`.settings-rail`) listing the visible section
  titles as `.nav-item` buttons (`role="tab"` is *not* required; keep them
  `<button>` with `aria-current="page"` to match the sidebar and existing tests).
- Only the **selected** section renders in the panel.
- The search box sits above the rail. Filtering narrows the rail; when the current
  selection is filtered out, auto-select the **first visible** section.
- When nothing matches, render the existing `EmptyState` ("No matching settings")
  in the panel and no rail items.
- Keep the `sr-only` live region and its exact text.

DOM order must be: `<header>` (with the `h1` `headingRef`) → search → rail → panel,
so `Shift+Tab` from the focused `h1` still leaves the screen.

---

## 8. Widget

Small, centred, elegant. Target ~300×64 logical.

- Pill shape: `--radius-pill`… use `border-radius: 12px`, hairline, background
  `color-mix(in srgb, var(--color-surface) 92%, transparent)`,
  `backdrop-filter: blur(12px)`, `--shadow-1`.
- Contents on one centred row: level meter (5 bars, 3px wide, 2px gap, max 18px
  tall) → copy (title `--fs-md` weight 500 + subtitle `--fs-xs` secondary,
  ellipsised) → elapsed `time` (`--fs-xs`, tabular-nums) → actions.
- Action buttons are 24px quiet buttons; Stop uses the accent.
- Padding `0 var(--space-3)`, gap `var(--space-3)`.
- Keep the scale/`--widget-scale` logic and every `aria-*`/live-region exactly.

---

## 9. Hard rules (do not break)

1. Do not rename any component, prop, CSS class already asserted by tests
   (`.nav-item`, `.me-*`), or any user-visible string.
2. Do not change accessible names, roles, `aria-*`, `role="status"`/`alert`
   live regions, focus management (`headingRef`), or keyboard behaviour.
3. Do not add dependencies. No Tailwind, no CSS-in-JS, no icon package.
4. No inline `style` attributes for static styling (the widget's dynamic
   `--widget-*` custom properties are the only exception).
5. Respect `prefers-reduced-motion` (already handled globally).
6. Keep every file passing `pnpm lint` (eslint incl. jsx-a11y) and
   `pnpm typecheck`. Prettier: 100 cols, single quotes, 2-space indent.
7. All colour pairs must reach WCAG AA (4.5:1 body, 3:1 for ≥15px semibold and
   for UI borders) in **both** themes.
