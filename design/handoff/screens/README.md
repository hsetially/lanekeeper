# CODING AGENTS: READ THIS FIRST

This is a **handoff bundle** from Claude Design (claude.ai/design).

A user mocked up designs in HTML/CSS/JS using an AI design tool, then exported this bundle so a coding agent can implement the designs for real.

## What you should do — IMPORTANT

**Find the primary design file under `lanekeeper-design-brief/project/` and read it top to bottom.** Then **follow its imports**: open every file it pulls in (shared components, CSS, scripts) so you understand how the pieces fit together before you start implementing.

**If anything is ambiguous, ask the user to confirm before you start implementing.** It's much cheaper to clarify scope up front than to build the wrong thing.

## About the design files

The design medium is **HTML/CSS/JS** — these are prototypes, not production code. Your job is to **recreate them pixel-perfectly** in whatever technology makes sense for the target codebase (React, Vue, native, whatever fits). Match the visual output; don't copy the prototype's internal structure unless it happens to fit.

**Don't render these files in a browser or take screenshots unless the user asks you to.** Everything you need — dimensions, colors, layout rules — is spelled out in the source. Read the HTML and CSS directly; a screenshot won't tell you anything they don't.

## Bundle contents

- `lanekeeper-design-brief/README.md` — this file
- `lanekeeper-design-brief/project/` — the `Lanekeeper design brief` project files (HTML prototypes, assets, components)

# Lanekeeper design system

Internal DevOps web app. It manages config on the NFS servers of Kubernetes test swimlanes (sit, sitb, sitc, presita, presitb, presitc). Base files come from `configuration-base-saas` (tags such as v2.3.1). Tenant overrides come from `csp-tenant-data` (branches sit1–sit13). A tenant file (`tx-infinity-core-sit1.yml`) sits beside its base file and wins when present. There is no chat UI.

Source: the written brief only. No codebase, Figma or logo was provided, so the product name is set in plain Inter type.

## Index
- `styles.css`: entry point (only `@import` lines)
- `tokens/fonts.css`: self-hosted Inter 400/500/600 and JetBrains Mono 400/500 (`assets/fonts/*.woff2`, SIL OFL)
- `tokens/colors.css`: semantic colours in shadcn/ui naming. Light theme in `:root`, dark in `.dark` / `[data-theme="dark"]`. Also status and diff tokens.
- `tokens/typography.css`, `tokens/spacing.css`: type scale, 4px grid, row heights, radius
- `assets/icons/*.svg`: Lucide icons, self-hosted
- `Lanekeeper Style Guide.dc.html`: living style guide with every token and component in light and dark. Includes a live WCAG contrast check, the shadcn mapping and the engineer token table.
- `guidelines/*.card.html`: foundation specimen cards
- `SKILL.md`: agent skill wrapper

## Content fundamentals
- Plain, specific and technical. Use the real names of swimlanes, branches, files and services ("sitc reads tx-infinity-core-sit1.yml").
- Sentence case everywhere. No exclamation marks. No emoji.
- Address the user as "you". Never use "we" for the system.
- Name consequences before actions: "2 services need restart", "File becomes NFS ahead until a PR merges".
- Errors state what happened, why, and the next step, plus a copyable request ID.
- Mono for every path, key, value, hash, branch and version.

## Visual foundations
- **Colour:** cool neutral greys plus one calm blue (`--primary`). shadcn's `--accent` is the hover surface, not the brand colour. Status colours appear only in status badges, banners and diff lines.
- **Status:** every status shows colour, icon and label. Fill weight also varies so states stay distinct in greyscale: Conflict is solid, Untracked is a dashed outline, Pending approval is an outline. Agent dots vary in shape: filled (online), ring (degraded), diamond (offline).
- **Type:** Inter, 13/18 body. Scale is 12/13/14/16/20/24. JetBrains Mono 12/18 for code. `tabular-nums` in tables.
- **Spacing:** 4px grid. Rows are fixed at 28px (compact) and 32px (default); tree rows are 28px, diff lines 18px. Controls are 28px (24px for sm).
- **Radius:** 4px for controls, badges and chips; 6px for cards, popovers and dialogs.
- **Elevation:** borders separate content. One shadow (`--shadow-popover`), used only by floating layers.
- **Backgrounds:** flat. No gradients, imagery or texture. The only pattern is the diagonal hatch on empty diff sides.
- **Motion:** 100–150ms ease-out for state changes only. Spinners use `loader-circle`. The reconnecting dot pulses. No bounces.
- **Hover:** one step darker (`--primary-hover`, `--secondary-hover`), or `--accent` for ghost and list rows. **Press:** the hover colour plus an inset shadow and a 1px drop.
- **Focus:** a 2px `--ring` outline with a 2px offset on every focusable element (`:focus-visible`). Inputs add a 1px inner ring.
- **Disabled:** 50% opacity and a not-allowed cursor. **Loading:** spinner plus `aria-busy`, and the label changes to a verb ("Working…").

## Iconography
Lucide (the shadcn/ui default), 2px stroke, self-hosted SVGs in `assets/icons/`. Rendered at 12px in badges, 14px in rows and buttons, 16px in headers. Icons are coloured via CSS `mask` + `currentColor`. They always pair with a text label or a tooltip. No emoji, and no unicode glyphs as icons (except ↑↓↵⌘⇧ inside kbd).

## Components (shadcn mapping)
Button/IconButton → button+tooltip · Input/SearchInput → input+kbd · Select → select · Combobox/MultiSelect → popover+command · Toggle → switch · Checkbox · Radio → radio-group · SegmentedControl → toggle-group · StatusBadge/RoleBadge/TierBadge → badge (cva variants) · Path/Setting/Hash/Branch chips, VersionLabel → badge+button · DataTable → table + TanStack Table + TanStack Virtual · TreeView → custom role=tree · Tabs · Breadcrumb · SplitPane → resizable · DiffHeader/DiffView → custom · CommandPalette → CommandDialog · Dialog · ConsequenceDialog/DestructiveConfirm → alert-dialog · Toast → sonner · Alert/Banner → alert · Kbd · ShortcutsSheet → dialog · Skeleton.

Intentional additions: TreeView, DiffView and LiveIndicator have no shadcn equivalent.

## Density
At 1440×900, with a 44px header, a 40px toolbar and a 32px table header, compact rows show 28 rows and default rows show 24.
