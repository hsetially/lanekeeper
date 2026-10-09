# Design reference screens

Put the Claude Design exports here as `<screen>.dc.html`, one file per screen or state. Use the names from the screen inventory, for example `swimlanes-overview.dc.html`, `file-view-diff.dc.html` or `adoption-wizard-step3.dc.html`.

- These files are **reference only**. Nothing in `web/` imports or serves them (S12).
- Put the "Engineering handoff" page next to them as `engineering-handoff.dc.html`, or as markdown.
- Commit them in a PR titled `design: handoff vN`. This folder is a contract owned by the design owner.
- Prompt 08 and the `lanekeeper-ui-from-design` skill describe how agents use them.
