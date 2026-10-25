# What a project lets an Application deploy (09/T6; S16): every resource a chart renders is allowed by the Argo CD project that deploys the
# chart, in the namespace it is in. Run with `conftest test --combine --namespace argocd_scope` over the rendered output of one chart and the
# file with the project(s) of the cluster it is deployed to. Without this, the first sync in the cluster stops at the first resource that
# the project does not know ("resource ... is not permitted in project"); with it, a chart that grows a kind is refused here, until
# the project and the approved list in argocd.rego grow it too, on purpose.
package argocd_scope

import rego.v1

import data.lk

projects := [p |
	some p in lk.of_kind("AppProject")
	startswith(p.apiVersion, "argoproj.io/")
]

# What the charts render: everything but the Argo CD files themselves.
rendered := [d |
	some d in lk.docs
	not d.kind in {"AppProject", "Application"}
]

group(d) := split(d.apiVersion, "/")[0] if contains(d.apiVersion, "/")

group(d) := "" if not contains(d.apiVersion, "/")

# The kinds that have no namespace. A kind that is not in this list is taken to be namespaced, and then the rule below asks for its
# namespace, so a cluster-scoped kind missing from the list is refused for the wrong reason, which names the document.
cluster_scoped := {
	"Namespace", "StorageClass", "PersistentVolume", "ClusterRole", "ClusterRoleBinding", "CustomResourceDefinition", "PriorityClass",
	"IngressClass", "ValidatingWebhookConfiguration", "MutatingWebhookConfiguration", "APIService", "RuntimeClass", "CSIDriver",
	"VolumeAttributesClass",
}

# An entry may name the one resource it allows (the Namespace of a project); without a name it allows the kind.
whitelisted(p, field, d) if {
	some w in object.get(p.spec, field, [])
	object.get(w, "group", "") == group(d)
	object.get(w, "kind", "") == d.kind
	object.get(w, "name", d.metadata.name) == d.metadata.name
}

allowed_in_namespace(d) if {
	some p in projects
	some dest in object.get(p.spec, "destinations", [])
	dest.namespace == d.metadata.namespace
	whitelisted(p, "namespaceResourceWhitelist", d)
}

allowed_cluster_wide(d) if {
	some p in projects
	whitelisted(p, "clusterResourceWhitelist", d)
}

deny contains msg if {
	some d in rendered
	not d.kind in cluster_scoped
	object.get(d.metadata, "namespace", "") == ""
	msg := sprintf("%s names no namespace, so the project that deploys it cannot be checked; every namespaced resource of a chart names its namespace", [lk.who(d)])
}

deny contains msg if {
	some d in rendered
	not d.kind in cluster_scoped
	ns := object.get(d.metadata, "namespace", "")
	ns != ""
	not allowed_in_namespace(d)
	msg := sprintf("%s in the namespace %q: no project allows the kind %s (group %q) in that namespace, and Argo CD would refuse to sync it", [lk.who(d), ns, d.kind, group(d)])
}

deny contains msg if {
	some d in rendered
	d.kind in cluster_scoped
	not allowed_cluster_wide(d)
	msg := sprintf("%s is cluster-scoped, and no project allows the kind %s (group %q) cluster-wide, so Argo CD would refuse to sync it", [lk.who(d), d.kind, group(d)])
}
