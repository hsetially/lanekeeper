//! The document model of the fixture generator.
//!
//! A [`Doc`] is a base file that evolves over three Git versions (the three commits of the base repo). It is stored as
//! a head, a list of chunks that each appear from some version on, and a tail, so any version can be rendered, and a
//! tenant fork can be "stale" in a precise way: it lacks the entries that were added after the version it was copied
//! from (check C1).

use super::rng::Rng;

/// The newest version: the head of the base repo.
pub const HEAD_VERSION: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
}

impl Eol {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Properties,
    Json,
    Xml,
    Xsl,
    Text,
    /// A file with no extension, `KEY=value` lines.
    Plain,
    Binary,
}

impl Format {
    /// True for formats that can take an appended `# comment` line without becoming invalid.
    #[must_use]
    pub fn takes_trailing_comment(self) -> bool {
        matches!(self, Self::Yaml | Self::Properties | Self::Text | Self::Plain)
    }
}

/// A block of lines that exists from version `since` on. `key` names it (an entry of a map, a property, ...).
#[derive(Debug, Clone)]
pub struct Chunk {
    pub since: u8,
    pub key: String,
    pub lines: Vec<String>,
}

impl Chunk {
    #[must_use]
    pub fn new(since: u8, key: &str, lines: Vec<String>) -> Self {
        Self {
            since,
            key: key.to_owned(),
            lines,
        }
    }
}

/// One base file.
#[derive(Debug, Clone)]
pub struct Doc {
    /// Path below `config/` (and below `data/config/` in the tenant repo, and on NFS).
    pub path: String,
    pub format: Format,
    pub eol: Eol,
    pub head: Vec<String>,
    pub chunks: Vec<Chunk>,
    pub tail: Vec<String>,
    /// Versions at which the file was touched without adding a chunk (a value changed).
    pub rev_versions: Vec<u8>,
    /// The first version that has the file.
    pub born: u8,
    /// Content of a binary file.
    pub binary: Vec<u8>,
    /// Top-level or nested keys that appear twice (check C8).
    pub dup_keys: Vec<String>,
    /// The file uses a YAML anchor and merge keys.
    pub anchors: bool,
}

impl Doc {
    #[must_use]
    pub fn text(
        path: &str,
        format: Format,
        eol: Eol,
        head: Vec<String>,
        chunks: Vec<Chunk>,
        tail: Vec<String>,
    ) -> Self {
        Self {
            path: path.to_owned(),
            format,
            eol,
            head,
            chunks,
            tail,
            rev_versions: Vec::new(),
            born: 1,
            binary: Vec::new(),
            dup_keys: Vec::new(),
            anchors: false,
        }
    }

    #[must_use]
    pub fn binary(path: &str, bytes: Vec<u8>) -> Self {
        let mut d = Self::text(path, Format::Binary, Eol::Lf, Vec::new(), Vec::new(), Vec::new());
        d.binary = bytes;
        d
    }

    #[must_use]
    pub fn exists_at(&self, version: u8) -> bool {
        self.born <= version
    }

    /// True when some chunk was added after version 1, so an old copy can lack entries.
    #[must_use]
    pub fn has_later_chunks(&self) -> bool {
        self.chunks.iter().any(|c| c.since > 1)
    }

    /// The newest version that adds a chunk.
    #[must_use]
    pub fn newest_chunk_version(&self) -> u8 {
        self.chunks.iter().map(|c| c.since).max().unwrap_or(1)
    }

    /// Keys of the chunks that a copy made at `version` lacks.
    #[must_use]
    pub fn missing_keys(&self, version: u8) -> Vec<String> {
        self.chunks
            .iter()
            .filter(|c| c.since > version)
            .map(|c| c.key.clone())
            .collect()
    }

    /// The file lines at `version`, without a tenant marker.
    fn marker_rev(&self, version: u8) -> Option<String> {
        let n = self.rev_versions.iter().filter(|&&v| v <= version).count();
        (n > 0).then(|| self.marker("revision", &n.to_string()))
    }

    /// One line of data that is not a comment where the format allows it, so a fork differs in meaning, not only in
    /// bytes.
    fn marker(&self, name: &str, value: &str) -> String {
        match self.format {
            Format::Yaml => format!("{name}: {value}"),
            Format::Properties => format!("{name}={value}"),
            Format::Plain => format!("{}={value}", name.to_uppercase()),
            Format::Json => format!("  \"{name}\": \"{value}\","),
            Format::Xml => format!("  <{name}>{value}</{name}>"),
            Format::Xsl => format!("  <!-- {name} {value} -->"),
            Format::Text | Format::Binary => format!("{name} {value}"),
        }
    }

    /// Renders the file as it is at `version`. With `tenant`, a marker line makes it a tenant-specific variant.
    #[must_use]
    pub fn render(&self, version: u8, tenant: Option<&str>) -> Vec<u8> {
        if self.format == Format::Binary {
            let mut bytes = self.binary.clone();
            if let Some(t) = tenant {
                // A tenant-specific image: same header, different payload.
                let salt = t.bytes().fold(0x5A_u8, |a, b| a.wrapping_add(b) | 1);
                for (i, b) in bytes.iter_mut().enumerate().skip(16) {
                    if i % 7 == 0 {
                        *b ^= salt;
                    }
                }
            }
            return bytes;
        }
        let mut lines: Vec<&str> =
            Vec::with_capacity(self.head.len() + self.chunks.len() * 4 + self.tail.len() + 3);
        lines.extend(self.head.iter().map(String::as_str));
        let rev = self.marker_rev(version);
        let marker = tenant.map(|t| self.marker("tenant", t));
        lines.extend(rev.as_deref());
        lines.extend(marker.as_deref());
        for c in self.chunks.iter().filter(|c| c.since <= version) {
            lines.extend(c.lines.iter().map(String::as_str));
        }
        lines.extend(self.tail.iter().map(String::as_str));
        let eol = self.eol.as_str();
        let mut out = String::with_capacity(lines.iter().map(|l| l.len() + 2).sum());
        for l in lines {
            out.push_str(l);
            out.push_str(eol);
        }
        out.into_bytes()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Vocabulary. Everything is synthetic: reserved example domains only (AGENTS.md rule 12).

pub const NOUNS: &[&str] = &[
    "account",
    "ledger",
    "teller",
    "branch",
    "routing",
    "session",
    "receipt",
    "statement",
    "currency",
    "limit",
    "hold",
    "journal",
    "queue",
    "gateway",
    "adapter",
    "channel",
    "profile",
    "rule",
    "schedule",
    "audit",
    "notification",
    "template",
    "mapping",
    "cache",
    "retry",
    "endpoint",
    "feature",
    "locale",
    "report",
    "settlement",
];

pub const KINDS: &[&str] = &[
    "config", "rules", "mapping", "profile", "settings", "routes", "limits", "flags", "options", "defaults",
];

const TIERS: &[&str] = &["dev", "sit", "uat", "prod"];
const SERVICES: &[&str] = &["video", "auth", "ledger", "notify", "docs", "rates", "fraud"];
const KNOWN_PLACEHOLDERS: &[&str] = &["CREATE_VIRTUAL_ITEM", "CORE_ROUTING", "LOG_LEVEL", "DB_POOL_SIZE"];
const ENV_PLACEHOLDERS: &[&str] = &["HOSTNAME", "POD_IP", "REGION"];

/// A value for a `key: value` line.
pub fn scalar(rng: &mut Rng) -> String {
    match rng.below(100) {
        0..=29 => (*rng.pick(NOUNS)).to_owned(),
        30..=49 => rng.range(1, 9_999).to_string(),
        50..=59 => if rng.chance(50) { "true" } else { "false" }.to_owned(),
        60..=74 => format!("${{{}}}", rng.pick(KNOWN_PLACEHOLDERS)),
        75..=79 => format!("${{{}}}", rng.pick(ENV_PLACEHOLDERS)),
        _ => format!(
            "https://{}.{}.corp.example/{}",
            rng.pick(SERVICES),
            rng.pick(TIERS),
            rng.pick(NOUNS)
        ),
    }
}

fn eol_for(rng: &mut Rng) -> Eol {
    if rng.chance(20) { Eol::CrLf } else { Eol::Lf }
}

fn dated_head(what: &str) -> Vec<String> {
    vec![format!("# synthetic fixture ({what}), not real configuration")]
}

// ---------------------------------------------------------------------------------------------------------------
// YAML

fn yaml_scalar_section(rng: &mut Rng, name: &str) -> Chunk {
    let mut lines = vec![format!("{name}:")];
    for i in 0..rng.range(2, 6) {
        lines.push(format!("  {}{i}: {}", rng.pick(NOUNS), scalar(rng)));
    }
    Chunk::new(1, name, lines)
}

fn yaml_list_section(rng: &mut Rng, name: &str) -> Chunk {
    let mut lines = vec![format!("{name}:")];
    for _ in 0..rng.range(2, 7) {
        lines.push(format!("  - {}", scalar(rng)));
    }
    Chunk::new(1, name, lines)
}

/// A map of entries. With `later`, the last entries only exist from version 3 (and the one before from version 2).
fn yaml_entry_map(
    rng: &mut Rng,
    parent: &str,
    entries: usize,
    later: bool,
    anchors: bool,
    tag: usize,
) -> Vec<Chunk> {
    let mut chunks = vec![Chunk::new(1, parent, vec![format!("{parent}:")])];
    for i in 0..entries {
        let key = format!("{}-{tag}x{i}", rng.pick(NOUNS));
        let mut lines = vec![format!("  {key}:")];
        if anchors {
            lines.push("    <<: *defaults".to_owned());
        }
        for j in 0..rng.range(2, 4) {
            lines.push(format!("    {}{j}: {}", rng.pick(NOUNS), scalar(rng)));
        }
        let since = if later && i + 1 == entries {
            3
        } else if later && i + 2 == entries && entries > 2 {
            2
        } else {
            1
        };
        chunks.push(Chunk::new(since, &key, lines));
    }
    chunks
}

/// Options for [`yaml_doc`].
#[derive(Debug, Clone, Copy, Default)]
pub struct YamlOptions {
    pub anchors: bool,
    pub duplicate_key: bool,
    pub later_entries: bool,
}

/// A YAML document of 20 to 120 lines.
pub fn yaml_doc(rng: &mut Rng, path: &str, opts: YamlOptions) -> Doc {
    let mut chunks = Vec::new();
    if opts.anchors {
        chunks.push(Chunk::new(
            1,
            "defaults",
            vec![
                "defaults: &defaults".to_owned(),
                "  timeoutMs: 3000".to_owned(),
                "  retries: 3".to_owned(),
            ],
        ));
    }
    let mut n = 0;
    for _ in 0..rng.range(1, 3) {
        let name = format!("{}{n}", rng.pick(NOUNS));
        chunks.push(yaml_scalar_section(rng, &name));
        n += 1;
    }
    if rng.chance(50) {
        let name = format!("{}List{n}", rng.pick(NOUNS));
        chunks.push(yaml_list_section(rng, &name));
        n += 1;
    }
    if opts.later_entries || opts.anchors || rng.chance(40) {
        let entries = if opts.later_entries {
            rng.range(3, 6)
        } else {
            rng.range(2, 8)
        };
        let parent = format!("{}Map{n}", rng.pick(NOUNS));
        chunks.extend(yaml_entry_map(
            rng,
            &parent,
            entries,
            opts.later_entries,
            opts.anchors,
            n,
        ));
    }
    let mut dup_keys = Vec::new();
    if opts.duplicate_key {
        chunks.insert(0, Chunk::new(1, "retryLimit", vec!["retryLimit: 3".to_owned()]));
        chunks.push(Chunk::new(1, "retryLimit", vec!["retryLimit: 5".to_owned()]));
        dup_keys.push("retryLimit".to_owned());
    }
    let mut doc = Doc::text(
        path,
        Format::Yaml,
        eol_for(rng),
        dated_head("yaml"),
        chunks,
        Vec::new(),
    );
    doc.dup_keys = dup_keys;
    doc.anchors = opts.anchors;
    doc
}

/// A YAML document of at least `lines` lines, with an anchor and merge keys (the "largest real file" shape).
pub fn big_yaml_doc(rng: &mut Rng, path: &str, lines: usize) -> Doc {
    let mut chunks = vec![Chunk::new(
        1,
        "defaults",
        vec![
            "defaults: &defaults".to_owned(),
            "  timeoutMs: 3000".to_owned(),
            "  retries: 3".to_owned(),
        ],
    )];
    chunks.push(Chunk::new(1, "catalog", vec!["catalog:".to_owned()]));
    let entries = lines / 6 + 1;
    for i in 0..entries {
        let key = format!("item-{i:05}");
        let since = if i + 1 == entries {
            3
        } else if i + 2 == entries {
            2
        } else {
            1
        };
        let body = vec![
            format!("  {key}:"),
            "    <<: *defaults".to_owned(),
            format!("    name: {}-{}", rng.pick(NOUNS), rng.range(1, 9_999)),
            format!(
                "    url: https://{}.{}.corp.example/{}",
                rng.pick(SERVICES),
                rng.pick(TIERS),
                rng.pick(NOUNS)
            ),
            format!("    enabled: {}", rng.chance(80)),
            format!("    weight: {}", rng.range(1, 100)),
        ];
        chunks.push(Chunk::new(since, &key, body));
    }
    let mut doc = Doc::text(
        path,
        Format::Yaml,
        Eol::CrLf,
        dated_head("large yaml"),
        chunks,
        Vec::new(),
    );
    doc.anchors = true;
    doc
}

// ---------------------------------------------------------------------------------------------------------------
// Other text formats

pub fn properties_doc(rng: &mut Rng, path: &str) -> Doc {
    let mut chunks = Vec::new();
    let n = rng.range(5, 40);
    for i in 0..n {
        let key = format!("{}.{}{i}", rng.pick(NOUNS), rng.pick(KINDS));
        let since = if i + 1 == n && rng.chance(30) { 3 } else { 1 };
        chunks.push(Chunk::new(since, &key, vec![format!("{key}={}", scalar(rng))]));
    }
    Doc::text(
        path,
        Format::Properties,
        eol_for(rng),
        dated_head("properties"),
        chunks,
        Vec::new(),
    )
}

pub fn plain_doc(rng: &mut Rng, path: &str) -> Doc {
    let mut chunks = Vec::new();
    for i in 0..rng.range(4, 25) {
        let key = format!("{}_{i}", rng.pick(NOUNS).to_uppercase());
        chunks.push(Chunk::new(1, &key, vec![format!("{key}={}", scalar(rng))]));
    }
    Doc::text(
        path,
        Format::Plain,
        eol_for(rng),
        dated_head("plain"),
        chunks,
        Vec::new(),
    )
}

pub fn json_doc(rng: &mut Rng, path: &str) -> Doc {
    let mut chunks = Vec::new();
    for i in 0..rng.range(4, 30) {
        let key = format!("{}{i}", rng.pick(NOUNS));
        chunks.push(Chunk::new(
            1,
            &key,
            vec![format!("  \"{key}\": \"{}\",", scalar(rng))],
        ));
    }
    let eol = eol_for(rng);
    Doc::text(
        path,
        Format::Json,
        eol,
        vec!["{".to_owned()],
        chunks,
        vec!["  \"enabled\": true".to_owned(), "}".to_owned()],
    )
}

pub fn xml_doc(rng: &mut Rng, path: &str) -> Doc {
    let mut chunks = Vec::new();
    for i in 0..rng.range(4, 30) {
        let key = format!("{}{i}", rng.pick(NOUNS));
        chunks.push(Chunk::new(
            1,
            &key,
            vec![format!("  <item name=\"{key}\">{}</item>", scalar(rng))],
        ));
    }
    let head = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_owned(),
        "<config>".to_owned(),
    ];
    Doc::text(
        path,
        Format::Xml,
        eol_for(rng),
        head,
        chunks,
        vec!["</config>".to_owned()],
    )
}

fn xsl_template(rng: &mut Rng, i: usize) -> Chunk {
    let key = format!("t{i}");
    let lines = vec![
        format!("  <xsl:template match=\"/doc/{}{i}\">", rng.pick(NOUNS)),
        format!("    <out id=\"{key}\"><xsl:value-of select=\"@v\"/></out>"),
        "  </xsl:template>".to_owned(),
    ];
    Chunk::new(1, &key, lines)
}

fn xsl_head() -> Vec<String> {
    vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_owned(),
        "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">".to_owned(),
    ]
}

pub fn xsl_doc(rng: &mut Rng, path: &str) -> Doc {
    let chunks = (0..rng.range(3, 25)).map(|i| xsl_template(rng, i)).collect();
    Doc::text(
        path,
        Format::Xsl,
        eol_for(rng),
        xsl_head(),
        chunks,
        vec!["</xsl:stylesheet>".to_owned()],
    )
}

/// A stylesheet of about `bytes` bytes.
pub fn huge_xsl_doc(rng: &mut Rng, path: &str, bytes: usize) -> Doc {
    let mut chunks = Vec::new();
    let mut size = 0;
    let mut i = 0;
    while size < bytes {
        let c = xsl_template(rng, i);
        size += c.lines.iter().map(|l| l.len() + 2).sum::<usize>();
        chunks.push(c);
        i += 1;
    }
    Doc::text(
        path,
        Format::Xsl,
        Eol::CrLf,
        xsl_head(),
        chunks,
        vec!["</xsl:stylesheet>".to_owned()],
    )
}

pub fn text_doc(rng: &mut Rng, path: &str) -> Doc {
    let mut chunks = Vec::new();
    for i in 0..rng.range(5, 60) {
        let words: Vec<&str> = (0..rng.range(4, 10)).map(|_| *rng.pick(NOUNS)).collect();
        chunks.push(Chunk::new(1, &format!("l{i}"), vec![words.join(" ")]));
    }
    Doc::text(
        path,
        Format::Text,
        eol_for(rng),
        dated_head("text"),
        chunks,
        Vec::new(),
    )
}

/// An image-like file: the right magic bytes followed by random data. `ext` is `png`, `bmp`, `gif` or `jpg`.
pub fn image_doc(rng: &mut Rng, path: &str, ext: &str) -> Doc {
    let magic: &[u8] = match ext {
        "png" => b"\x89PNG\r\n\x1a\n",
        "bmp" => b"BM",
        "gif" => b"GIF89a",
        _ => b"\xff\xd8\xff\xe0",
    };
    let mut bytes = magic.to_vec();
    let n = rng.range(300, 6_000);
    bytes.extend(rng.bytes(n));
    Doc::binary(path, bytes)
}
