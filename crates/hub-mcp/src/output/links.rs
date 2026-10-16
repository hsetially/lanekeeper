//! Web links (A12): the page of the web UI that shows the full result, or where a person confirms a draft.
//!
//! A link is `origin` + a route template filled with percent-encoded components. The origin is the configured public origin and
//! nothing a caller sent, so a link never leaves the hub; a link longer than the contract's 512 characters is not made (`None`).
//! The route shapes are prompt 08's; the defaults here follow the handoff (`/swimlanes/:id/files?path=...`, `/drafts/:id`) and
//! every one can be replaced in configuration (`LinkTemplates`).

use domain::{DraftId, LogicalFile, NfsPath, SwimlaneId};

use crate::config::{ConfigError, PublicOrigin};

/// The longest link: the contract's `web_link` (`maxLength` 512).
pub const MAX_LINK_LEN: usize = 512;

/// The route templates. A `{name}` is replaced by the percent-encoded value; the names each template may use are in
/// [`LinkTemplates::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTemplates {
    pub swimlane: String,
    pub file: String,
    pub drift: String,
    pub findings: String,
    pub restarts: String,
    pub history: String,
    pub compare: String,
    pub compare_tree: String,
    pub grid: String,
    pub search_text: String,
    pub search_settings: String,
    pub docs_search: String,
    pub doc: String,
    pub effective: String,
    pub served: String,
    pub draft: String,
    pub pr: String,
}

impl Default for LinkTemplates {
    fn default() -> Self {
        let t = |s: &str| s.to_owned();
        Self {
            swimlane: t("/swimlanes/{swimlane}"),
            file: t("/swimlanes/{swimlane}/files?path={path}"),
            drift: t("/swimlanes/{swimlane}/drift"),
            findings: t("/findings?swimlane={swimlane}"),
            restarts: t("/swimlanes/{swimlane}/restarts"),
            history: t("/swimlanes/{swimlane}/history?path={path}"),
            compare: t("/compare?left={left}&right={right}&file={file}"),
            compare_tree: t("/compare?left={left}&right={right}"),
            grid: t("/grid?file={file}&swimlanes={swimlanes}"),
            search_text: t("/search?mode=text&q={q}"),
            search_settings: t("/search?mode=settings&q={q}"),
            docs_search: t("/docs?q={q}"),
            doc: t("/docs/view?path={path}"),
            effective: t("/swimlanes/{swimlane}/effective?file={file}&tenant={tenant}"),
            served: t("/swimlanes/{swimlane}/served?application={application}&tenant={tenant}&file={path}"),
            draft: t("/drafts/{id}"),
            pr: t("/swimlanes/{swimlane}/pull-requests"),
        }
    }
}

const PLACEHOLDERS: &[&str] = &[
    "swimlane",
    "path",
    "left",
    "right",
    "file",
    "swimlanes",
    "q",
    "tenant",
    "application",
    "id",
];

impl LinkTemplates {
    fn all(&self) -> [&String; 17] {
        [
            &self.swimlane,
            &self.file,
            &self.drift,
            &self.findings,
            &self.restarts,
            &self.history,
            &self.compare,
            &self.compare_tree,
            &self.grid,
            &self.search_text,
            &self.search_settings,
            &self.docs_search,
            &self.doc,
            &self.effective,
            &self.served,
            &self.draft,
            &self.pr,
        ]
    }

    /// # Errors
    ///
    /// [`ConfigError::Invalid`] for a template that does not start with `/`, is longer than 200 bytes, holds a character that is
    /// not printable ASCII, or names a placeholder that does not exist.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for template in self.all() {
            if !template.starts_with('/')
                || template.starts_with("//")
                || template.len() > 200
                || !template.bytes().all(|b| (0x21..=0x7e).contains(&b))
            {
                return Err(ConfigError::Invalid("link_templates"));
            }
            let mut rest = template.as_str();
            while let Some(open) = rest.find('{') {
                let after = &rest[open + 1..];
                let close = after.find('}').ok_or(ConfigError::Invalid("link_templates"))?;
                if !PLACEHOLDERS.contains(&&after[..close]) {
                    return Err(ConfigError::Invalid("link_templates"));
                }
                rest = &after[close + 1..];
            }
        }
        Ok(())
    }
}

/// Percent-encode a path or query component: everything but `A-Za-z0-9-._~` (and `/` in a path, kept readable).
fn encode(component: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(component.len() + 8);
    for byte in component.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (keep_slash && byte == b'/')
        {
            out.push(char::from(byte));
        } else {
            const UPPER: &[u8; 16] = b"0123456789ABCDEF";
            out.push('%');
            out.push(char::from(UPPER[usize::from(byte >> 4)]));
            out.push(char::from(UPPER[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// Builds the links of a result.
#[derive(Debug, Clone)]
pub struct WebLinks {
    origin: PublicOrigin,
    templates: LinkTemplates,
}

impl WebLinks {
    #[must_use]
    pub fn new(origin: PublicOrigin, templates: LinkTemplates) -> Self {
        Self { origin, templates }
    }

    fn fill(&self, template: &str, values: &[(&str, &str)]) -> Option<String> {
        let mut path = String::with_capacity(template.len() + 64);
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            path.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let close = after.find('}')?;
            let name = &after[..close];
            let (_, value) = values.iter().find(|(n, _)| *n == name)?;
            // A path keeps its slashes; a value in a query string does not.
            let in_query = path.contains('?');
            path.push_str(&encode(value, !in_query && name == "path"));
            rest = &after[close + 1..];
        }
        path.push_str(rest);
        let link = self.origin.url(&path);
        (link.len() <= MAX_LINK_LEN).then_some(link)
    }

    #[must_use]
    pub fn swimlane(&self, s: &SwimlaneId) -> Option<String> {
        self.fill(&self.templates.swimlane, &[("swimlane", s.as_str())])
    }

    #[must_use]
    pub fn file(&self, s: &SwimlaneId, path: &NfsPath) -> Option<String> {
        self.fill(
            &self.templates.file,
            &[("swimlane", s.as_str()), ("path", path.as_str())],
        )
    }

    #[must_use]
    pub fn drift(&self, s: &SwimlaneId) -> Option<String> {
        self.fill(&self.templates.drift, &[("swimlane", s.as_str())])
    }

    #[must_use]
    pub fn findings(&self, s: Option<&SwimlaneId>) -> Option<String> {
        match s {
            Some(s) => self.fill(&self.templates.findings, &[("swimlane", s.as_str())]),
            None => self.fill("/findings", &[]),
        }
    }

    #[must_use]
    pub fn restarts(&self, s: &SwimlaneId) -> Option<String> {
        self.fill(&self.templates.restarts, &[("swimlane", s.as_str())])
    }

    #[must_use]
    pub fn history(&self, s: &SwimlaneId, path: &NfsPath) -> Option<String> {
        self.fill(
            &self.templates.history,
            &[("swimlane", s.as_str()), ("path", path.as_str())],
        )
    }

    #[must_use]
    pub fn compare(&self, left: &str, right: &str, file: &LogicalFile) -> Option<String> {
        self.fill(
            &self.templates.compare,
            &[("left", left), ("right", right), ("file", file.as_str())],
        )
    }

    #[must_use]
    pub fn compare_tree(&self, left: &str, right: &str) -> Option<String> {
        self.fill(&self.templates.compare_tree, &[("left", left), ("right", right)])
    }

    #[must_use]
    pub fn grid(&self, file: &LogicalFile, swimlanes: &[&str]) -> Option<String> {
        self.fill(
            &self.templates.grid,
            &[("file", file.as_str()), ("swimlanes", &swimlanes.join(","))],
        )
    }

    #[must_use]
    pub fn search_text(&self, pattern: &str) -> Option<String> {
        self.fill(&self.templates.search_text, &[("q", pattern)])
    }

    #[must_use]
    pub fn search_settings(&self, q: &str) -> Option<String> {
        self.fill(&self.templates.search_settings, &[("q", q)])
    }

    #[must_use]
    pub fn docs_search(&self, q: &str) -> Option<String> {
        self.fill(&self.templates.docs_search, &[("q", q)])
    }

    #[must_use]
    pub fn doc(&self, path: &str) -> Option<String> {
        self.fill(&self.templates.doc, &[("path", path)])
    }

    #[must_use]
    pub fn effective(&self, s: &SwimlaneId, file: &LogicalFile, tenant: &str) -> Option<String> {
        self.fill(
            &self.templates.effective,
            &[
                ("swimlane", s.as_str()),
                ("file", file.as_str()),
                ("tenant", tenant),
            ],
        )
    }

    #[must_use]
    pub fn served(&self, s: &SwimlaneId, application: &str, tenant: &str, path: &NfsPath) -> Option<String> {
        self.fill(
            &self.templates.served,
            &[
                ("swimlane", s.as_str()),
                ("application", application),
                ("tenant", tenant),
                ("path", path.as_str()),
            ],
        )
    }

    #[must_use]
    pub fn draft(&self, id: &DraftId) -> Option<String> {
        self.fill(&self.templates.draft, &[("id", id.as_str())])
    }

    #[must_use]
    pub fn pull_requests(&self, s: &SwimlaneId) -> Option<String> {
        self.fill(&self.templates.pr, &[("swimlane", s.as_str())])
    }
}
