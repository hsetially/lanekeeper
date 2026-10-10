//! Release hints from Helm labels (Q19, D84).
//!
//! The tenant data is deployed as a Helm chart (`csp-tenant-data-<branch>`), and Helm labels what it renders. The agent
//! reads exactly two labels, `helm.sh/chart` and `app.kubernetes.io/instance`, and only on objects whose chart label
//! matches the configured globs. It never reads annotations, values or any other label, and it sends the label values
//! as they are: working out which tenant or release they mean is the hub's job.

use std::collections::BTreeMap;

use domain::ShortText;
use globset::{Glob, GlobSet, GlobSetBuilder};

use super::error::BuildError;

pub const CHART_LABEL: &str = "helm.sh/chart";
pub const INSTANCE_LABEL: &str = "app.kubernetes.io/instance";

/// The two Helm label values of an object that matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelmHints {
    pub chart: String,
    pub instance: Option<String>,
}

/// Decides which objects carry a hint.
#[derive(Debug, Clone)]
pub struct HelmFilter {
    charts: GlobSet,
}

impl HelmFilter {
    pub fn new(chart_globs: &[ShortText]) -> Result<Self, BuildError> {
        let mut builder = GlobSetBuilder::new();
        for glob in chart_globs {
            let glob = Glob::new(glob.as_str()).map_err(|_| BuildError::BadGlob {
                setting: "LK_HELM_HINT_CHART_GLOBS",
            })?;
            builder.add(glob);
        }
        let charts = builder.build().map_err(|_| BuildError::BadGlob {
            setting: "LK_HELM_HINT_CHART_GLOBS",
        })?;
        Ok(Self { charts })
    }

    /// The hints of an object with these labels, or `None` if its chart label is missing or does not match. A value the
    /// hub would reject (over 256 bytes, control characters) is a miss too.
    pub fn hints(&self, labels: Option<&BTreeMap<String, String>>) -> Option<HelmHints> {
        let labels = labels?;
        let chart = labels.get(CHART_LABEL)?;
        if !self.charts.is_match(chart) || ShortText::parse(chart).is_err() {
            return None;
        }
        let instance = labels
            .get(INSTANCE_LABEL)
            .filter(|v| ShortText::parse(v).is_ok())
            .cloned();
        Some(HelmHints {
            chart: chart.clone(),
            instance,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(globs: &[&str]) -> HelmFilter {
        let globs: Vec<ShortText> = globs.iter().map(|g| ShortText::parse(g).unwrap()).collect();
        HelmFilter::new(&globs).unwrap()
    }

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_matching_chart_gives_both_labels() {
        let f = filter(&["csp-tenant-data-*"]);
        let hints = f
            .hints(Some(&labels(&[
                (CHART_LABEL, "csp-tenant-data-sit1-0.3.1"),
                (INSTANCE_LABEL, "tenant-data-sit1"),
                ("app", "ignored"),
            ])))
            .unwrap();
        assert_eq!(hints.chart, "csp-tenant-data-sit1-0.3.1");
        assert_eq!(hints.instance.as_deref(), Some("tenant-data-sit1"));
    }

    #[test]
    fn the_instance_label_is_optional() {
        let f = filter(&["csp-tenant-data-*"]);
        let hints = f
            .hints(Some(&labels(&[(CHART_LABEL, "csp-tenant-data-sit1-1.0.0")])))
            .unwrap();
        assert_eq!(hints.instance, None);
    }

    #[test]
    fn a_chart_that_does_not_match_gives_nothing() {
        let f = filter(&["csp-tenant-data-*"]);
        assert_eq!(
            f.hints(Some(&labels(&[(CHART_LABEL, "ingress-nginx-4.0.0")]))),
            None
        );
        assert_eq!(
            f.hints(Some(&labels(&[(INSTANCE_LABEL, "csp-tenant-data-sit1")]))),
            None
        );
        assert_eq!(f.hints(Some(&BTreeMap::new())), None);
        assert_eq!(f.hints(None), None);
    }

    #[test]
    fn no_globs_means_no_hints() {
        let f = filter(&[]);
        assert_eq!(
            f.hints(Some(&labels(&[(CHART_LABEL, "csp-tenant-data-sit1-1.0.0")]))),
            None
        );
    }

    #[test]
    fn a_bad_glob_is_a_build_error() {
        let globs = [ShortText::parse("[unclosed").unwrap()];
        assert!(matches!(HelmFilter::new(&globs), Err(BuildError::BadGlob { .. })));
    }
}
