# Third-party skills

Vendored, not installed as plugins, so every contributor and both agents (Claude Code and Cursor, through `.cursor/skills`) get the same pinned set. Each source commit is at least 14 days old (AGENTS.md rule 15). Files under `.claude/skills/**` are CODEOWNERS-protected: changing, adding or updating a skill needs human review.

The seven `lanekeeper-*` skills always win on any conflict. Third-party skills must not change the plan, commit or PR protocol in AGENTS.md.

| Skill | Source | Pinned commit | Used for |
|---|---|---|---|
| `shadcn` | shadcn-ui/ui `skills/shadcn` | `98a1fe67b439` (2026-09-21) | Prompt 08: shadcn/ui work |
| `skill-creator` | anthropics/skills | `33375500bcea` (2026-09-24) | Prompt 11 and maintaining skills and their evals |
| `webapp-testing` | anthropics/skills | `33375500bcea` (2026-09-24) | Prompt 08: Playwright checks |
| `systematic-debugging`, `verification-before-completion`, `test-driven-development`, `receiving-code-review` | obra/superpowers | `8ca22dba9a94` (2026-09-25) | All prompts: debugging, evidence before "done", tests, acting on review findings |
| `m06-error-handling`, `m07-concurrency`, `m10-performance`, `m15-anti-pattern`, `domain-fintech`, `domain-cloud-native` | actionbook/rust-skills | `5c40d3ad785d` (2026-08-23) | Rust prompts; skills-only, no plugin hooks |
| `writing-for-agents` | mattpocock/skills `skills/productivity` | `2aecca12ea9f` (2026-09-24) | Editing AGENTS.md and skills |

Licences: anthropics/skills items keep their `LICENSE.txt` (Apache-2.0).

## Local changes

- **shadcn:** the CLI is pinned to `shadcn@4.21.0` (released 2026-09-04) and run as `pnpm dlx shadcn@4.21.0`, never `@latest`. Removed: the `allowed-tools` pre-approval, the `!` command that ran the CLI when the skill loaded, the chat rules (`rules/chat.md` and every chat reference; Lanekeeper has no chat UI), the Codex agent file, and the images and evals.
- **superpowers:** removed the pressure-test and creation-log files from `systematic-debugging`. The skills' cross-references to `superpowers:*` names still read correctly; only the four vendored skills exist.
- **rust-skills:** the "see also" lines name sibling skills that are not vendored (m01, m09 and others). Ignore them. These skills are advice on Rust style; they are written for older editions in places, so `lanekeeper-rust-conventions`, edition 2024 and the workspace lints take precedence.
- **skill-creator:** its eval viewer HTML loads Google Fonts and a SheetJS script from a CDN, and its scripts run `claude -p`. Use it on a developer machine only, never in CI.

## Not vendored (decided against)

ui-ux-pro-max (design comes from the Claude Design handoff), lobehub (chat platform; no chat UI here), vibe-kanban (sunsetting), graphify (optional later), and the rest of superpowers, mattpocock/skills and rust-skills (their plan, spec and ticket flows clash with AGENTS.md).

## Updating

Pick a newer commit that is at least 14 days old, diff it against the vendored copy, re-apply the local changes above, update this table, and open a PR.
