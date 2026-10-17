//! `app_candidates` (T6, P7): the grid reads only the application names that a logical file's name can ask about, and not every
//! service's name. That is only right while the engine's rule for a property source asks about nothing else, so this holds the engine's
//! `logical_file_role` to the list: with all the applications and with only the candidates among them, the role is the same.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{AppName, FileRole, LogicalFile};
use engine::logical_file_role;
use hub_registry::read::effective::app_candidates;
use proptest::prelude::*;

/// A piece of a name: the words that applications and files are made of in a real fleet, and the ones the rule treats specially.
fn word() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "application",
        "core",
        "core-routing",
        "pay",
        "pay-gateway",
        "ledger",
        "sit1",
        "uat",
        "x",
    ])
}

fn file_name() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(word(), 1..4),
        prop::sample::select(vec![".yml", ".yaml", ".properties", ".json", ".txt", ""]),
        prop::option::of(word()),
    )
        .prop_map(|(words, ext, folder)| {
            let name = format!("{}{ext}", words.join("-"));
            match folder {
                Some(f) => format!("{f}/{name}"),
                None => name,
            }
        })
}

fn app_names() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        (prop::collection::vec(word(), 1..4)).prop_map(|w| w.join("-")),
        0..8,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn the_role_of_a_file_depends_only_on_the_candidates_among_the_applications(
        name in file_name(),
        apps in app_names(),
    ) {
        let Ok(file) = LogicalFile::parse(&name) else { return Ok(()) };
        let all: Vec<AppName> = apps.iter().filter_map(|a| AppName::parse(a).ok()).collect();
        let candidates = app_candidates(&file);
        let some: Vec<AppName> = all
            .iter()
            .filter(|a| candidates.iter().any(|c| c == a.as_str()))
            .cloned()
            .collect();
        prop_assert_eq!(
            logical_file_role(&file, &all),
            logical_file_role(&file, &some),
            "{} with {:?} and the candidates {:?}", name, apps, candidates
        );
    }
}

#[test]
fn an_application_with_the_name_of_the_stem_makes_the_file_a_property_source() {
    let file = LogicalFile::parse("config/core-routing.yml").unwrap();
    let apps = [AppName::parse("core-routing").unwrap()];
    assert_eq!(logical_file_role(&file, &[]), FileRole::Resource);
    assert_eq!(logical_file_role(&file, &apps), FileRole::PropertySource);
    let candidates = app_candidates(&file);
    assert!(candidates.contains(&"core-routing".to_owned()), "{candidates:?}");
    assert!(candidates.contains(&"core".to_owned()), "{candidates:?}");
}

#[test]
fn the_candidates_are_few_sorted_and_without_repeats() {
    let file = LogicalFile::parse("a/b-c-d.e.yml").unwrap();
    let candidates = app_candidates(&file);
    assert!(candidates.windows(2).all(|w| w[0] < w[1]), "{candidates:?}");
    assert!(candidates.len() <= 8, "{candidates:?}");
}
