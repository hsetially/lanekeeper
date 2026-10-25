# Storage policy (09/T3; S10). Run with `conftest test --combine --namespace storage`. Every persistent volume is encrypted with
# a customer-managed key (CMEK): its claim names a StorageClass that is rendered next to it and carries a Cloud KMS key. The
# chart's fallback for a cluster without a key (plan 09 A11b, Q29) is the cluster's default class and the annotation
# `lanekeeper.io/cmek: fallback` on the claim; that renders with a warning, and `deploy/tests/render.sh` makes the warning a
# failure for prod and staging. The buckets are added by 09/T5.
package storage

import rego.v1

import data.lk

cmek_classes := {c.metadata.name |
	some c in lk.of_kind("StorageClass")
	key := object.get(c, ["parameters", "disk-encryption-kms-key"], "")
	key != ""
}

# Every claim in the output: a standalone PersistentVolumeClaim, or a StatefulSet's claim template.
claims := array.concat(
	[{"owner": lk.who(c), "claim": c} | some c in lk.of_kind("PersistentVolumeClaim")],
	[{"owner": lk.who(s), "claim": t} | some s in lk.of_kind("StatefulSet"); some t in object.get(s.spec, "volumeClaimTemplates", [])],
)

fallback(claim) if object.get(claim.metadata, ["annotations", "lanekeeper.io/cmek"], "") == "fallback"

deny contains msg if {
	some x in claims
	not object.get(x.claim.spec, "storageClassName", "") in cmek_classes
	not fallback(x.claim)
	msg := sprintf("%s, claim %s: its StorageClass is not one with a customer-managed key (disk-encryption-kms-key) rendered with it (S10)", [x.owner, x.claim.metadata.name])
}

warn contains msg if {
	some x in claims
	not object.get(x.claim.spec, "storageClassName", "") in cmek_classes
	fallback(x.claim)
	msg := sprintf("%s, claim %s: Google-managed encryption, not a customer-managed key (S10, Q29): set storage.cmekKey", [x.owner, x.claim.metadata.name])
}
