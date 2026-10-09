//! The base file catalog: the files of `configuration-base-saas` (`config/...`) at design shape.
//!
//! It plants the shapes the engine has to cope with: channel folders, nested service folders, CRLF files, YAML anchors,
//! duplicate keys, XSL, images, extension-less files, 6,000-line YAML, and the same file name in several non-channel
//! folders (check C9). Names are globally unique except for the planted duplicates, so the duplicate list in the
//! manifest is exact.

use std::collections::BTreeSet;

use super::content::{
    Chunk, Doc, Eol, Format, KINDS, NOUNS, YamlOptions, big_yaml_doc, huge_xsl_doc, image_doc, json_doc,
    plain_doc, properties_doc, text_doc, xml_doc, xsl_doc, yaml_doc,
};
use super::rng::Rng;

/// Everything the other modules need to know about the base repo.
#[derive(Debug)]
pub struct Catalog {
    pub docs: Vec<Doc>,
    pub channels: Vec<String>,
    /// `parent/channel` folders, in the order of `channels.yml`.
    pub channel_folders: Vec<String>,
    /// Names that appear in more than one non-channel folder, with the folders.
    pub duplicate_names: Vec<(String, Vec<String>)>,
}

/// Size knobs, see [`super::Scale`].
#[derive(Debug, Clone, Copy)]
pub struct CatalogParams {
    pub base_files: usize,
    pub adapters: usize,
    pub dup_names: usize,
    pub big_yaml: usize,
    pub huge_xsl_bytes: usize,
}

const NAMED_FOLDERS: &[&str] = &[
    "tx-infinity-api",
    "holds",
    "ui",
    "document-service",
    "security",
    "limits",
    "jwt-proxy-injector-service",
    "reference-data",
    "reports",
    "notifications",
    "payments",
    "accounts",
];

const CHANNELS: &[&str] = &["remote-itm-teller", "atm-iso", "atm"];
const CHANNEL_FOLDERS: &[(&str, &str)] = &[
    ("ui", "remote-itm-teller"),
    ("tx-infinity-api", "remote-itm-teller"),
    ("holds", "remote-itm-teller"),
    ("holds", "atm-iso"),
    ("limits", "atm"),
];

struct Builder<'a> {
    rng: &'a mut Rng,
    docs: Vec<Doc>,
    used_names: BTreeSet<String>,
    used_paths: BTreeSet<String>,
}

impl Builder<'_> {
    fn push(&mut self, doc: Doc) {
        let name = doc.path.rsplit('/').next().unwrap_or_default().to_owned();
        self.used_names.insert(name);
        self.used_paths.insert(doc.path.clone());
        self.docs.push(doc);
    }

    /// A name no other file has, with the extension for `format`.
    fn unique_name(&mut self, format: Format, ext_hint: &str) -> String {
        for attempt in 0..40 {
            let noun = *self.rng.pick(NOUNS);
            let kind = *self.rng.pick(KINDS);
            let stem = if format == Format::Plain {
                format!("mappings{}{}E2ETest", capitalise(noun), capitalise(kind))
            } else {
                format!("{noun}-{kind}")
            };
            let stem = if attempt > 20 {
                format!("{stem}{attempt}")
            } else {
                stem
            };
            let ext = match format {
                Format::Yaml | Format::Binary => ext_hint,
                Format::Properties => "properties",
                Format::Json => "json",
                Format::Xml => "xml",
                Format::Xsl => "xsl",
                Format::Text => "txt",
                Format::Plain => "",
            };
            let name = if ext.is_empty() {
                stem
            } else {
                format!("{stem}.{ext}")
            };
            if !self.used_names.contains(&name) {
                return name;
            }
        }
        // 40 collisions in a vocabulary of 300 combinations: fall back to a counter.
        format!("generated-{}.yml", self.used_names.len())
    }

    fn make(&mut self, folder: &str, format: Format) -> Doc {
        let ext = match format {
            Format::Yaml => {
                if self.rng.chance(70) {
                    "yml"
                } else {
                    "yaml"
                }
            }
            Format::Binary => *self.rng.pick(&["png", "bmp", "gif", "jpg"]),
            _ => "",
        };
        let name = self.unique_name(format, ext);
        let path = join(folder, &name);
        self.make_at(&path, format, &name)
    }

    fn make_at(&mut self, path: &str, format: Format, name: &str) -> Doc {
        let rng = &mut *self.rng;
        let mut doc = match format {
            Format::Yaml => {
                let opts = YamlOptions {
                    anchors: rng.chance(8),
                    duplicate_key: rng.chance(2),
                    later_entries: rng.chance(20),
                };
                yaml_doc(rng, path, opts)
            }
            Format::Properties => properties_doc(rng, path),
            Format::Json => json_doc(rng, path),
            Format::Xml => xml_doc(rng, path),
            Format::Xsl => xsl_doc(rng, path),
            Format::Text => text_doc(rng, path),
            Format::Plain => plain_doc(rng, path),
            Format::Binary => image_doc(rng, path, name.rsplit('.').next().unwrap_or("png")),
        };
        // History: some files appear later, some are revised without a new entry.
        if rng.chance(8) {
            doc.born = 2;
        } else if rng.chance(6) {
            doc.born = 3;
        }
        if doc.format != Format::Binary {
            doc.rev_versions = match rng.below(10) {
                0 => vec![2],
                1 => vec![3],
                2 => vec![2, 3],
                _ => Vec::new(),
            };
            for c in &mut doc.chunks {
                c.since = c.since.max(doc.born);
            }
            if doc.born > 1 {
                doc.rev_versions.clear();
            }
        }
        doc
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// A fixed document with its own history.
fn fixed(path: &str, format: Format, eol: Eol, head: &[&str], chunks: Vec<Chunk>, tail: &[&str]) -> Doc {
    let to_vec = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    Doc::text(path, format, eol, to_vec(head), chunks, to_vec(tail))
}

fn lines(key: &str, since: u8, body: &[&str]) -> Chunk {
    Chunk::new(since, key, body.iter().map(|s| (*s).to_owned()).collect())
}

const NOTE: &str = "# synthetic fixture, not real configuration";

/// Root files: the channel list and the property sources (`CORE_ROUTING`, `LOG_LEVEL` are the config-server's own
/// test values).
fn root_specials() -> Vec<Doc> {
    vec![
        fixed(
            "channels.yml",
            Format::Yaml,
            Eol::Lf,
            &[NOTE],
            vec![lines(
                "channels",
                1,
                &["channels:", "  - remote-itm-teller", "  - atm-iso", "  - atm"],
            )],
            &[],
        ),
        fixed(
            "application.properties",
            Format::Properties,
            Eol::Lf,
            &[NOTE],
            vec![
                lines("CREATE_VIRTUAL_ITEM", 1, &["CREATE_VIRTUAL_ITEM = true"]),
                lines("CORE_ROUTING", 1, &["CORE_ROUTING = false"]),
                lines("LOG_LEVEL", 1, &["LOG_LEVEL: INFO"]),
            ],
            &[],
        ),
        fixed(
            "application.yml",
            Format::Yaml,
            Eol::Lf,
            &[NOTE],
            vec![
                lines("server", 1, &["server:", "  port: 8080"]),
                lines("DB_POOL_SIZE", 1, &["DB_POOL_SIZE: 10"]),
            ],
            &[],
        ),
    ]
}

/// The `tx-infinity-api` examples: a placeholder file (rendering) and a map that gained an `atm` entry in version 3 (C1).
fn tx_infinity_specials(rng: &mut Rng) -> Vec<Doc> {
    let mut out = vec![
        fixed(
            "tx-infinity-api/tx-infinity-core.yml",
            Format::Yaml,
            Eol::CrLf,
            &[NOTE],
            vec![
                lines("coreRouting", 1, &["coreRouting: ${CORE_ROUTING}"]),
                lines("logLevel", 1, &["logLevel: ${LOG_LEVEL}"]),
                lines(
                    "txInfinityOptions",
                    1,
                    &[
                        "txInfinityOptions:",
                        "  remote-itm-teller:",
                        "    enableAccountSorting: true",
                    ],
                ),
                lines("callback", 1, &["callback: ${CALLBACK_URL}"]),
            ],
            &[],
        ),
        fixed(
            "tx-infinity-api/entryGroupConfig.yml",
            Format::Yaml,
            Eol::CrLf,
            &[NOTE],
            vec![
                lines("entryGroupConfigMap", 1, &["entryGroupConfigMap:"]),
                lines("teller", 1, &["  teller:", "    groups: 4"]),
                lines("branch", 1, &["  branch:", "    groups: 2"]),
                lines("atm", 3, &["  atm:", "    groups: 1"]),
            ],
            &[],
        ),
    ];
    for name in ["account-sorting-config.yml", "miniStatementConfig.yml"] {
        out.push(yaml_doc(
            rng,
            &format!("tx-infinity-api/{name}"),
            YamlOptions::default(),
        ));
    }
    out
}

/// A file that repeats three keys (C8), a CRLF file, an image, an extension-less file and a channel file.
fn other_specials(rng: &mut Rng) -> Vec<Doc> {
    let role =
        |name: &str, level: &str| lines(name, 1, &[&format!("  {name}:"), &format!("    level: {level}")]);
    let mut roles = fixed(
        "security/security-roles.yml",
        Format::Yaml,
        Eol::Lf,
        &[NOTE],
        vec![
            lines("roles", 1, &["roles:"]),
            role("txRemoteJuniorTeller", "1"),
            role("txRemoteSeniorTeller", "2"),
            role("txRemoteSupervisor", "3"),
            role("txRemoteJuniorTeller", "1"),
            role("txRemoteSeniorTeller", "2"),
            role("txRemoteSupervisor", "3"),
        ],
        &[],
    );
    roles.dup_keys = [
        "txRemoteJuniorTeller",
        "txRemoteSeniorTeller",
        "txRemoteSupervisor",
    ]
    .map(str::to_owned)
    .to_vec();

    let mut limits = yaml_doc(rng, "limits/limit-profiles.yml", YamlOptions::default());
    limits.eol = Eol::CrLf;
    let mut plain = plain_doc(rng, "jwt-proxy-injector-service/mappingsItemEvaluationE2ETest");
    plain.eol = Eol::Lf;
    vec![
        roles,
        limits,
        image_doc(rng, "document-service/resources/receipt-ci_1.bmp", "bmp"),
        plain,
        yaml_doc(rng, "ui/ui-common-config.yaml", YamlOptions::default()),
        yaml_doc(
            rng,
            "ui/remote-itm-teller/ui-common-config.yaml",
            YamlOptions::default(),
        ),
    ]
}

/// The folders files are spread over.
struct Folders {
    adapters: Vec<String>,
    /// Every folder, named ones repeated so they get more files (the channel folders need parents with plenty).
    weighted: Vec<String>,
}

impl Folders {
    fn plan(p: &CatalogParams) -> Self {
        let mut all: Vec<String> = NAMED_FOLDERS.iter().map(|s| (*s).to_owned()).collect();
        let adapters: Vec<String> = (1..=p.adapters)
            .map(|i| format!("core-adapters/adapter-{i:02}"))
            .collect();
        all.extend(adapters.iter().cloned());
        all.extend((1..=(p.base_files / 120).max(2)).map(|i| format!("ui/widgets/w-{i:02}")));
        for i in 1..=(p.base_files / 14).max(2) {
            all.push(if i % 3 == 0 {
                format!("svc-{i:03}/resources")
            } else {
                format!("svc-{i:03}")
            });
        }
        let mut weighted = all.clone();
        for f in all.iter().take(NAMED_FOLDERS.len()) {
            weighted.extend(std::iter::repeat_n(f.clone(), 6));
        }
        Self { adapters, weighted }
    }

    fn pick(&self, rng: &mut Rng) -> String {
        rng.pick(&self.weighted).clone()
    }
}

impl Builder<'_> {
    /// The same name in several adapter folders, never in a channel folder (C9).
    fn plant_duplicates(&mut self, p: &CatalogParams, adapters: &[String]) -> Vec<(String, Vec<String>)> {
        let mut planted = Vec::new();
        for k in 0..p.dup_names {
            let (name, count) = match k {
                0 => ("authentication-config.yaml".to_owned(), p.adapters.clamp(2, 12)),
                1 => ("error-codes-config.yaml".to_owned(), p.adapters.clamp(2, 4)),
                _ => (format!("shared-{}-{k:02}.yaml", self.rng.pick(NOUNS)), 2 + k % 2),
            };
            let chosen: Vec<String> = adapters
                .iter()
                .cycle()
                .skip(k)
                .take(count.min(adapters.len()))
                .cloned()
                .collect();
            for f in &chosen {
                let doc = self.make_at(&join(f, &name), Format::Yaml, &name);
                self.push(doc);
            }
            planted.push((name, chosen));
        }
        planted
    }

    /// The 6,000-line YAML files and the stylesheet of over 1 MiB (the largest real files).
    fn add_big_files(&mut self, p: &CatalogParams) {
        for i in 0..p.big_yaml {
            let lines = 6_000 + self.rng.below(400);
            let doc = big_yaml_doc(
                self.rng,
                &format!("reference-data/catalog-big-{:02}.yml", i + 1),
                lines,
            );
            self.push(doc);
        }
        let huge = huge_xsl_doc(self.rng, "reports/statement-transform-big.xsl", p.huge_xsl_bytes);
        self.push(huge);
    }

    /// The format mix of the remaining files, spread over the folders.
    fn fill(&mut self, p: &CatalogParams, folders: &Folders) {
        let remaining = p
            .base_files
            .saturating_sub(self.docs.len() + CHANNEL_FOLDERS.len() * 3);
        let pct = |n: usize, min: usize| (p.base_files * n / 100).max(min);
        let mut mix: Vec<Format> = Vec::new();
        mix.extend(std::iter::repeat_n(Format::Binary, pct(2, 2)));
        mix.extend(std::iter::repeat_n(Format::Xsl, pct(4, 2)));
        mix.extend(std::iter::repeat_n(Format::Plain, pct(3, 2)));
        mix.extend(std::iter::repeat_n(Format::Properties, pct(9, 2)));
        mix.extend(std::iter::repeat_n(Format::Json, pct(4, 1)));
        mix.extend(std::iter::repeat_n(Format::Xml, pct(4, 1)));
        mix.extend(std::iter::repeat_n(Format::Text, pct(3, 1)));
        mix.truncate(remaining);
        mix.resize(remaining, Format::Yaml);
        self.rng.shuffle(&mut mix);
        for format in mix {
            let folder = folders.pick(self.rng);
            let doc = self.make(&folder, format);
            self.push(doc);
        }
    }

    /// Channel copies: the same file name as a file of the parent folder, in `<parent>/<channel>/`.
    fn add_channel_copies(&mut self) -> Vec<String> {
        let mut channel_folders = Vec::new();
        for (parent, channel) in CHANNEL_FOLDERS {
            let folder = format!("{parent}/{channel}");
            channel_folders.push(folder.clone());
            let parents: Vec<(String, Format)> = self
                .docs
                .iter()
                .filter(|d| {
                    d.path.rsplit_once('/').is_some_and(|(dir, _)| dir == *parent)
                        && d.format != Format::Binary
                        && d.path != "ui/ui-common-config.yaml"
                })
                .map(|d| (d.path.rsplit('/').next().unwrap_or_default().to_owned(), d.format))
                .collect();
            let want = self.rng.range(2, 4).min(parents.len());
            let mut idx: Vec<usize> = (0..parents.len()).collect();
            self.rng.shuffle(&mut idx);
            for i in idx.into_iter().take(want) {
                let (name, format) = &parents[i];
                let path = join(&folder, name);
                if !self.used_paths.contains(&path) {
                    let doc = self.make_at(&path, *format, name);
                    self.push(doc);
                }
            }
        }
        channel_folders
    }
}

/// Builds the catalog. Deterministic for a given `rng` stream and parameters.
pub fn build(rng: &mut Rng, p: &CatalogParams) -> Catalog {
    let mut special_rng = rng.fork("specials");
    let mut b = Builder {
        rng,
        docs: Vec::new(),
        used_names: BTreeSet::new(),
        used_paths: BTreeSet::new(),
    };
    for d in root_specials()
        .into_iter()
        .chain(tx_infinity_specials(&mut special_rng))
        .chain(other_specials(&mut special_rng))
    {
        b.push(d);
    }
    let folders = Folders::plan(p);
    let duplicate_names = b.plant_duplicates(p, &folders.adapters);
    b.add_big_files(p);
    b.fill(p, &folders);
    let channel_folders = b.add_channel_copies();

    // Top up with plain YAML to the exact size (the channel copies may have been fewer than reserved).
    while b.docs.len() < p.base_files {
        let folder = folders.pick(b.rng);
        let doc = b.make(&folder, Format::Yaml);
        b.push(doc);
    }
    debug_assert_eq!(b.docs.len(), p.base_files, "more fixed files than base_files");

    let mut docs = b.docs;
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    Catalog {
        docs,
        channels: CHANNELS.iter().map(|s| (*s).to_owned()).collect(),
        channel_folders,
        duplicate_names,
    }
}
