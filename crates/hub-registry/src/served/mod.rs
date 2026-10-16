//! The nightly "as served" verification (T10, D86, A36): what the engine says a service is given, checked against what the config-server
//! really serves.
//!
//! D86 has the engine render resource files "the way the config-server does" and says that the agent can fetch exactly what a service receives
//! (`HubCommand::FetchServed`, read-only, rate-limited) "for nightly differential checks of the engine. A mismatch is a high-severity finding".
//! This module is that check. It is made of two parts:
//!
//! - **the sample** (this file): which combinations of `(application, tenant, channel, file)` a night looks at. A swimlane has thousands of
//!   resource files, a handful of tenants and a few channels, and the agent answers five requests a second at most, so the check looks at
//!   [`RegistryConfig::served_verify_sample`](crate::config::RegistryConfig::served_verify_sample) combinations (200) of each swimlane. They
//!   are chosen **deterministically**: the same swimlane, the same files and the same night choose the same combinations, whatever the
//!   replica, so a failure can be repeated, and a test can say which they are. The night changes the choice (`round`), so that over the
//!   nights every combination is looked at; the combinations that have an open finding are looked at **every** night first, so that a
//!   finding is cleared the night after the config-server is right again;
//! - **the job** ([`verify_job`]): `registry-served-verify`, a leased job. For each swimlane whose agent is connected it asks the agent, one
//!   request at a time, for the sampled combinations; compares what comes back with the engine's rendering ([`crate::read`]'s effective
//!   config, the same code the REST route and the MCP tool use); and raises `FindingKind::ServedMismatch` (C12, high) for each one that
//!   differs, naming the file and the tenant and the channel, and clears it when it matches again.
//!
//! # What is compared, and what is not
//!
//! **Resource files** only (D82): the config-server serves one of them whole, with the `${KEY}` it knows replaced, and the engine's effective
//! config of a resource is the hash of exactly that rendering. A property source has no such thing: its effective config is the merge of
//! its settings, which the config-server's file endpoint does not serve, so there is nothing of the engine's to hold against the file that
//! comes back. Property-source merges are therefore **not** verified as served (a residual risk in the threat model).
//!
//! The population leaves out: a denied file, and any file that has a denied copy (D79: the agent would refuse it, and the hub does not ask);
//! a file in a channel folder (a request names the channel, and the file is asked for as the channel's copy of the file outside it); a file
//! over `max_file_bytes` (the engine does not render it, and the agent refuses an answer over 2 MiB).
//!
//! A swimlane with an open C11 finding is **skipped**: the config-server's list of folders and channels is the one from its last start
//! (D83), C11 says that a restart is due, and until it happens the server and the files differ by design: a C12 would only repeat C11 for
//! every file of the folder.
//!
//! # What a finding says
//!
//! Names, counts and ids (S10, S14): the application, the tenant, the channel and the file, the hash of the engine's rendering and the
//! hash of what was served, and whether the file was served at all. No file content, no setting value and no environment value is in it,
//! in a log line, in a metric or in an event: the served text is hashed in memory and dropped, and a mismatch does not say *where* the
//! texts differ (that would be a diff of content, and the "As served" view and a compare already give it to a person who may read it).
//!
//! # Trust
//!
//! What the agent answers is data from a cluster the hub does not control. A hostile or broken agent can make the answer for its own
//! swimlane anything: it can raise a C12 that is not true (a wrong hash) or hide one that is (a right hash); it cannot do either for
//! another swimlane, because the request goes to the agent of the swimlane and the finding is written for that swimlane only. The check is
//! a smoke alarm for drift between the engine and the config-server, not a proof, and the threat model says so.

pub mod verify_job;

use std::collections::BTreeSet;

use domain::{AppName, ChannelName, LogicalFile, SwimlaneId, TenantId};
use sha2::{Digest, Sha256};

use crate::pipeline::c9_c11::inputs::FileRow;
use crate::read::resource::application_of;

pub use self::verify_job::{JOB_NAME, ServedDeps, ServedReport, ServedVerifier};

/// One request the check may make: a file as a service of an application, in a tenant and a channel, would ask the config-server for it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Combination {
    pub application: AppName,
    pub tenant: TenantId,
    pub channel: Option<ChannelName>,
    pub file: LogicalFile,
}

impl Combination {
    /// What names the combination among the others of its swimlane: the identity of its finding. The file is last, so that no application,
    /// tenant or channel name (their characters are `[A-Za-z0-9._-]`) can run into it.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.application,
            self.tenant,
            self.channel.as_ref().map_or("", ChannelName::as_str),
            self.file
        )
    }
}

/// The combinations a swimlane has, as the checks would ask for them. Built once for a run from the swimlane's files, tenants,
/// applications and channels; it does not hold the files' content, and not even every combination: [`nth`](Self::nth) makes one from its
/// number, so a swimlane of 100,000 files in 5 tenants and 20 channels costs the size of its file list, not of its product.
#[derive(Debug, Clone, Default)]
pub struct Population {
    /// The resource files outside the channel folders, sorted: asked for with no channel.
    files: Vec<LogicalFile>,
    /// The resource files that a channel folder has a copy of, sorted: asked for in that channel. Where the channel has no copy the
    /// answer is the one of no channel, which the first list already asks for.
    pairs: Vec<(LogicalFile, ChannelName)>,
    tenants: Vec<TenantId>,
    apps: Vec<AppName>,
}

/// The file `a/chan/x.yml` of channel `chan` is the channel's copy of `a/x.yml`: the logical file outside the channel folder, when `file`
/// is directly in the folder of one of the `channels`.
fn channel_copy_of(file: &LogicalFile, channels: &[ChannelName]) -> Option<(LogicalFile, ChannelName)> {
    let (dir, name) = file.as_str().rsplit_once('/')?;
    let (outer, last) = dir.rsplit_once('/').map_or(("", dir), |(o, l)| (o, l));
    let channel = channels.iter().find(|c| c.as_str() == last)?;
    let logical = if outer.is_empty() {
        name.to_owned()
    } else {
        format!("{outer}/{name}")
    };
    Some((LogicalFile::parse(&logical).ok()?, channel.clone()))
}

impl Population {
    /// The population of a swimlane. `files` are its files on NFS as the structure checks read them; `deny` says whether one is denied;
    /// `channels` are the ones `channels.yml` lists; `apps` are the applications its Deployments run (D88).
    pub(crate) fn new(
        files: &[FileRow],
        deny: &dyn Fn(&FileRow) -> bool,
        max_bytes: u64,
        tenants: Vec<TenantId>,
        apps: Vec<AppName>,
        channels: &[ChannelName],
    ) -> Self {
        // A logical file with a denied copy is out as a whole: the config-server resolves a request to one of its copies, and the hub does
        // not know which until it asks.
        let mut denied: BTreeSet<&LogicalFile> = BTreeSet::new();
        for f in files {
            if deny(f) {
                denied.insert(&f.logical);
            }
        }
        let mut base: BTreeSet<LogicalFile> = BTreeSet::new();
        let mut pairs: BTreeSet<(LogicalFile, ChannelName)> = BTreeSet::new();
        for f in files {
            if f.role != domain::FileRole::Resource || denied.contains(&f.logical) || f.size > max_bytes {
                continue;
            }
            if engine::is_channel_dir(&f.path.parent(), channels) {
                if let Some(pair) = channel_copy_of(&f.logical, channels) {
                    pairs.insert(pair);
                }
            } else {
                base.insert(f.logical.clone());
            }
        }
        // A channel's copy is asked for only when its file is not denied either, which `denied` holds the channel copies' logical files
        // out of (they are the copies' own logical files, `a/chan/x.yml`): the outer file is checked in its own right.
        let pairs: Vec<(LogicalFile, ChannelName)> = pairs
            .into_iter()
            .filter(|(outer, _)| !denied.contains(outer))
            .collect();
        let mut tenants = tenants;
        tenants.sort();
        tenants.dedup();
        let mut apps = apps;
        apps.sort();
        apps.dedup();
        Self {
            files: base.into_iter().collect(),
            pairs,
            tenants,
            apps,
        }
    }

    /// How many combinations there are.
    pub fn len(&self) -> u64 {
        (self.files.len() as u64)
            .saturating_add(self.pairs.len() as u64)
            .saturating_mul(self.tenants.len() as u64)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn make(
        &self,
        file: &LogicalFile,
        tenant: &TenantId,
        channel: Option<&ChannelName>,
    ) -> Option<Combination> {
        Some(Combination {
            application: application_of(file, &self.apps)?,
            tenant: tenant.clone(),
            channel: channel.cloned(),
            file: file.clone(),
        })
    }

    /// The combination with this number (`0..len()`), the files without a channel first, then the files in a channel; each file in the
    /// order of the tenants.
    pub fn nth(&self, i: u64) -> Option<Combination> {
        let tenants = u64::try_from(self.tenants.len()).ok().filter(|t| *t > 0)?;
        let (slot, t) = (i / tenants, usize::try_from(i % tenants).ok()?);
        let tenant = self.tenants.get(t)?;
        let base = u64::try_from(self.files.len()).ok()?;
        if slot < base {
            self.make(self.files.get(usize::try_from(slot).ok()?)?, tenant, None)
        } else {
            let (file, channel) = self.pairs.get(usize::try_from(slot - base).ok()?)?;
            self.make(file, tenant, Some(channel))
        }
    }

    /// Whether this combination is one the swimlane has now.
    pub fn contains(&self, c: &Combination) -> bool {
        if !self.tenants.contains(&c.tenant)
            || application_of(&c.file, &self.apps).as_ref() != Some(&c.application)
        {
            return false;
        }
        match &c.channel {
            None => self.files.binary_search(&c.file).is_ok(),
            Some(channel) => self
                .pairs
                .binary_search(&(c.file.clone(), channel.clone()))
                .is_ok(),
        }
    }
}

/// The seed of a swimlane's sample on a night: no replica, run or process enters it.
fn seed(swimlane: &SwimlaneId, round: u64) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"lanekeeper/served-verify/v1\n");
    h.update((swimlane.as_str().len() as u64).to_be_bytes());
    h.update(swimlane.as_str().as_bytes());
    h.update(round.to_be_bytes());
    h.finalize().into()
}

/// The `j`-th draw of a seed, as a number in `0..total`.
fn draw(seed: &[u8; 32], j: u64, total: u64) -> u64 {
    let mut h = Sha256::new();
    h.update(seed);
    h.update(j.to_be_bytes());
    let d = h.finalize();
    let mut eight = [0_u8; 8];
    eight.copy_from_slice(&d[..8]);
    u64::from_be_bytes(eight) % total.max(1)
}

/// How many draws are made before the sample is filled in order: a bound on the work, never reached by a population much larger than the
/// sample, and a population just over it needs about `n ln n` draws, far below.
const DRAWS_PER_SLOT: u64 = 64;

/// The combinations to ask for in a swimlane on a night: first the ones that have an open finding and are still combinations of the
/// swimlane (`pinned`), then draws from a pseudo-random sequence of the swimlane's id and the night until there are `n` in all, or every
/// combination if there are fewer. Distinct, sorted, and the same every time for the same input.
pub fn sample(
    population: &Population,
    swimlane: &SwimlaneId,
    round: u64,
    pinned: &[Combination],
    n: usize,
) -> Vec<Combination> {
    let n = n.max(1);
    let mut chosen: BTreeSet<Combination> = pinned
        .iter()
        .filter(|c| population.contains(c))
        .take(n)
        .cloned()
        .collect();
    let total = population.len();
    if total <= u64::try_from(n).unwrap_or(u64::MAX) {
        // Every combination fits: all of them (the pinned ones are among them).
        return (0..total).filter_map(|i| population.nth(i)).collect();
    }
    let seed = seed(swimlane, round);
    let mut taken: BTreeSet<u64> = BTreeSet::new();
    let limit = u64::try_from(n)
        .unwrap_or(u64::MAX)
        .saturating_mul(DRAWS_PER_SLOT);
    let mut j = 0;
    while chosen.len() < n && j < limit {
        let i = draw(&seed, j, total);
        j += 1;
        if taken.insert(i) {
            if let Some(c) = population.nth(i) {
                chosen.insert(c);
            }
        }
    }
    // The draws did not fill the sample (a population just over `n`, and bad luck): the rest in order, so that the sample is always `n`.
    let mut i = 0;
    while chosen.len() < n && i < total {
        if let Some(c) = population.nth(i) {
            chosen.insert(c);
        }
        i += 1;
    }
    chosen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use domain::{ContentHash, DriftState, FileClass, FileKind, FileRole, NfsPath};
    use proptest::prelude::*;

    use super::*;

    fn row(path: &str, role: FileRole) -> FileRow {
        let p = NfsPath::parse(path).unwrap();
        FileRow {
            logical: LogicalFile::from_nfs(p.clone()).unwrap(),
            path: p,
            kind: FileKind::Base,
            class: FileClass::Text,
            state: DriftState::InSync,
            role,
            blob: ContentHash::from_bytes([1; 32]),
            denied: false,
            size: 10,
        }
    }

    fn tenant(t: &str) -> TenantId {
        TenantId::parse(t).unwrap()
    }

    fn channel(c: &str) -> ChannelName {
        ChannelName::parse(c).unwrap()
    }

    fn population(files: &[FileRow], tenants: &[&str], channels: &[&str]) -> Population {
        let channels: Vec<ChannelName> = channels.iter().map(|c| channel(c)).collect();
        Population::new(
            files,
            &|f: &FileRow| f.denied,
            1 << 20,
            tenants.iter().map(|t| tenant(t)).collect(),
            vec![AppName::parse("core").unwrap()],
            &channels,
        )
    }

    fn many(n: usize) -> Vec<FileRow> {
        (0..n)
            .map(|i| row(&format!("core/f{i:04}.txt"), FileRole::Resource))
            .collect()
    }

    #[test]
    fn only_resource_files_outside_the_channel_folders_are_asked_for_without_a_channel() {
        let files = vec![
            row("core/msg.txt", FileRole::Resource),
            row("core/app.yml", FileRole::PropertySource),
            row("core/chan/msg.txt", FileRole::Resource),
        ];
        let p = population(&files, &["sit1"], &["chan"]);
        let all: Vec<Combination> = (0..p.len()).filter_map(|i| p.nth(i)).collect();
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(
            all.iter()
                .any(|c| c.channel.is_none() && c.file.as_str() == "core/msg.txt")
        );
        assert!(
            all.iter()
                .any(|c| c.channel.as_ref().is_some_and(|c| c.as_str() == "chan")
                    && c.file.as_str() == "core/msg.txt"),
            "the channel's copy is asked for as the file outside the folder, in the channel: {all:?}"
        );
        assert!(
            all.iter()
                .all(|c| c.file.as_str() != "core/app.yml" && c.file.as_str() != "core/chan/msg.txt"),
            "a property source is not compared, and a channel folder is no file of its own"
        );
        assert!(all.iter().all(|c| c.application.as_str() == "core"));
    }

    #[test]
    fn a_file_with_a_denied_copy_is_not_asked_for_and_neither_is_its_channel_copy() {
        let mut denied_copy = row("core/msg-sit1.txt", FileRole::Resource);
        denied_copy.logical = LogicalFile::parse("core/msg.txt").unwrap();
        denied_copy.denied = true;
        let files = vec![
            row("core/msg.txt", FileRole::Resource),
            denied_copy,
            row("core/other.txt", FileRole::Resource),
            row("core/chan/msg.txt", FileRole::Resource),
        ];
        let p = population(&files, &["sit1"], &["chan"]);
        let all: Vec<Combination> = (0..p.len()).filter_map(|i| p.nth(i)).collect();
        assert!(
            all.iter().all(|c| c.file.as_str() != "core/msg.txt"),
            "a file with a denied copy is out as a whole, in every channel: {all:?}"
        );
        assert!(all.iter().any(|c| c.file.as_str() == "core/other.txt"));
    }

    #[test]
    fn a_file_over_the_size_the_engine_renders_is_not_asked_for() {
        let mut big = row("core/big.txt", FileRole::Resource);
        big.size = (1 << 20) + 1;
        let p = population(&[big, row("core/small.txt", FileRole::Resource)], &["sit1"], &[]);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn the_numbers_run_over_the_files_the_pairs_and_the_tenants_and_every_combination_is_contained() {
        let files = vec![
            row("core/a.txt", FileRole::Resource),
            row("core/b.txt", FileRole::Resource),
            row("core/chan/a.txt", FileRole::Resource),
        ];
        let p = population(&files, &["sit1", "sit2", "sit3"], &["chan"]);
        assert_eq!(p.len(), (2 + 1) * 3);
        let all: Vec<Combination> = (0..p.len()).filter_map(|i| p.nth(i)).collect();
        assert_eq!(all.len(), 9);
        let distinct: BTreeSet<&Combination> = all.iter().collect();
        assert_eq!(distinct.len(), 9, "every number is a combination of its own");
        assert!(all.iter().all(|c| p.contains(c)));
        assert!(p.nth(p.len()).is_none());
        let stranger = Combination {
            application: AppName::parse("core").unwrap(),
            tenant: tenant("sit9"),
            channel: None,
            file: LogicalFile::parse("core/a.txt").unwrap(),
        };
        assert!(!p.contains(&stranger));
    }

    #[test]
    fn a_swimlane_without_tenants_or_resource_files_has_no_combination() {
        assert!(population(&many(5), &[], &[]).is_empty());
        assert!(population(&[row("core/a.yml", FileRole::PropertySource)], &["sit1"], &[]).is_empty());
    }

    #[test]
    fn the_sample_is_two_hundred_distinct_combinations_of_a_larger_population() {
        let p = population(&many(100), &["sit1", "sit2", "sit3"], &[]);
        assert_eq!(p.len(), 300);
        let s = sample(&p, &SwimlaneId::parse("sit1").unwrap(), 7, &[], 200);
        assert_eq!(s.len(), 200);
        assert_eq!(s.iter().collect::<BTreeSet<_>>().len(), 200);
        assert!(s.iter().all(|c| p.contains(c)));
    }

    #[test]
    fn a_population_that_fits_is_asked_for_whole() {
        let p = population(&many(10), &["sit1", "sit2"], &[]);
        assert_eq!(
            sample(&p, &SwimlaneId::parse("sit1").unwrap(), 1, &[], 200).len(),
            20
        );
    }

    #[test]
    fn the_same_swimlane_and_night_choose_the_same_sample_and_another_night_another() {
        let p = population(&many(400), &["sit1", "sit2"], &[]);
        let lane = SwimlaneId::parse("sit1").unwrap();
        let a = sample(&p, &lane, 5, &[], 200);
        assert_eq!(a, sample(&p, &lane, 5, &[], 200));
        assert_ne!(a, sample(&p, &lane, 6, &[], 200), "the night changes the choice");
        assert_ne!(
            a,
            sample(&p, &SwimlaneId::parse("sit2").unwrap(), 5, &[], 200),
            "and so does the swimlane"
        );
    }

    #[test]
    fn over_the_nights_every_combination_is_asked_for() {
        let p = population(&many(60), &["sit1", "sit2"], &[]);
        let lane = SwimlaneId::parse("sit1").unwrap();
        let mut seen: BTreeSet<Combination> = BTreeSet::new();
        for round in 0..40 {
            seen.extend(sample(&p, &lane, round, &[], 20));
        }
        assert_eq!(seen.len(), 120, "forty nights of twenty cover 120 combinations");
    }

    #[test]
    fn a_combination_with_a_finding_is_asked_for_every_night_and_one_that_is_gone_is_not() {
        let p = population(&many(300), &["sit1"], &[]);
        let lane = SwimlaneId::parse("sit1").unwrap();
        let pinned = p.nth(299).unwrap();
        let gone = Combination {
            file: LogicalFile::parse("core/deleted.txt").unwrap(),
            ..pinned.clone()
        };
        for round in 0..5 {
            let s = sample(&p, &lane, round, &[pinned.clone(), gone.clone()], 50);
            assert_eq!(s.len(), 50);
            assert!(s.contains(&pinned), "night {round}");
            assert!(!s.contains(&gone));
        }
    }

    #[test]
    fn more_findings_than_the_sample_fill_it_with_findings_and_no_more() {
        let p = population(&many(300), &["sit1"], &[]);
        let pinned: Vec<Combination> = (0..60).filter_map(|i| p.nth(i)).collect();
        let s = sample(&p, &SwimlaneId::parse("sit1").unwrap(), 1, &pinned, 50);
        assert_eq!(s.len(), 50);
    }

    #[test]
    fn a_population_just_over_the_sample_still_fills_it() {
        let p = population(&many(21), &["sit1"], &[]);
        for round in 0..20 {
            let s = sample(&p, &SwimlaneId::parse("sit1").unwrap(), round, &[], 20);
            assert_eq!(s.len(), 20, "round {round}");
        }
    }

    #[test]
    fn the_key_of_a_combination_names_all_four_parts_and_no_part_runs_into_another() {
        let c = Combination {
            application: AppName::parse("core").unwrap(),
            tenant: tenant("sit1"),
            channel: Some(channel("web")),
            file: LogicalFile::parse("core/msg.txt").unwrap(),
        };
        assert_eq!(c.key(), "core|sit1|web|core/msg.txt");
        let none = Combination {
            channel: None,
            ..c.clone()
        };
        assert_eq!(none.key(), "core|sit1||core/msg.txt");
        assert_ne!(c.key(), none.key());
    }

    proptest! {
        /// For any population and any night: the sample is exactly `min(n, population)` distinct combinations of the population, and
        /// asking again gives the same.
        #[test]
        fn the_sample_is_min_of_n_and_the_population_distinct_inside_it_and_repeatable(
            files in 0usize..120, tenants in 0usize..5, n in 1usize..80, round in any::<u64>(),
        ) {
            let names: Vec<String> = (0..tenants).map(|t| format!("t{t}")).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let p = population(&many(files), &refs, &[]);
            let lane = SwimlaneId::parse("lane").unwrap();
            let s = sample(&p, &lane, round, &[], n);
            let expected = usize::try_from(p.len()).unwrap().min(n);
            prop_assert_eq!(s.len(), expected);
            prop_assert_eq!(s.iter().collect::<BTreeSet<_>>().len(), expected);
            prop_assert!(s.iter().all(|c| p.contains(c)));
            prop_assert_eq!(&s, &sample(&p, &lane, round, &[], n));
        }
    }
}
