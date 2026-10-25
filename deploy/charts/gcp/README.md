# gcp chart

The Google Cloud resources of Lanekeeper as Config Connector manifests (plan 09 A4). **09/T3 starts it with Cloud Armor;** T5 adds
the buckets and keys, T6 Binary Authorization. Secret containers only, never values.

`templates/cloudarmor.yaml` renders `ComputeSecurityPolicy/<cloudArmor.policyName>` (the hub chart's `gateway.securityPolicyName`):

| Priority | Allows | Rule |
|---|---|---|
| 1000.. | the users' GlobalProtect egress ranges (Q2), any path | `SRC_IPS_V1`, ten ranges a rule |
| 2000.. | GitHub's webhook ranges (Q28), the exact path `/hooks/github` and nothing else | `request.path == '/hooks/github' && (inIpRange(origin.ip, '...') \|\| ...)`, four ranges a rule |
| 2147483647 | nobody | `deny(403)` |

`deploy/policy/network.rego` checks the rendering: the hooks expression must be exactly that shape, GitHub's ranges may appear in no
other rule, every range GitHub lists must be in a hooks rule, no allowed range may be the internet, and the default must deny.

## GitHub's ranges are generated, never typed

`deploy/values/hub/github-hook-ranges.yaml` is written by `node deploy/tools/github-hook-ranges.mjs --write` from the `hooks` list of
`https://api.github.com/meta` and layered over the values (`-f github-hook-ranges.yaml -f prod.yaml`). The nightly job runs
`--check`. **Until a person runs `--write` with network access the file is empty and the chart refuses to render**, which is how
the missing step shows (plan 09 blockers, B5).
