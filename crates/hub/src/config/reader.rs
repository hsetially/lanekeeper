//! The one way settings are read (S7, S10): a value comes from the lookup, is checked, and on any problem the error names the
//! variable and never the value.
//!
//! A group of settings (`limits`, `database`, `git`, `gcp`, `web_tls`) takes a [`Reader`] and returns its own typed struct, so a
//! group cannot reach the environment by another route and a new group cannot forget the rules below.

use std::io;
use std::net::SocketAddr;
use std::path::Path;

use super::ConfigError;

/// Reads variables through a lookup (the environment in `main`, a map in tests) and files through a reader (the disk in
/// `main`, a map in tests).
pub struct Reader<'a> {
    lookup: &'a dyn Fn(&str) -> Option<String>,
    read_file: &'a dyn Fn(&Path) -> io::Result<String>,
}

impl std::fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader").finish_non_exhaustive()
    }
}

impl<'a> Reader<'a> {
    pub fn new(
        lookup: &'a dyn Fn(&str) -> Option<String>,
        read_file: &'a dyn Fn(&Path) -> io::Result<String>,
    ) -> Self {
        Self { lookup, read_file }
    }

    /// The value, or `None` when the variable is unset or empty.
    pub fn get(&self, name: &'static str) -> Option<String> {
        (self.lookup)(name).filter(|v| !v.is_empty())
    }

    /// Like [`Reader::get`], and a value with a control character is never valid: it would make a log line ambiguous.
    pub fn parse(&self, name: &'static str) -> Result<Option<String>, ConfigError> {
        let Some(value) = self.get(name) else {
            return Ok(None);
        };
        if value.chars().any(char::is_control) {
            return Err(ConfigError::Invalid(name));
        }
        Ok(Some(value))
    }

    pub fn addr(&self, name: &'static str, default: &str) -> Result<SocketAddr, ConfigError> {
        self.parse(name)?
            .unwrap_or_else(|| default.to_owned())
            .parse()
            .map_err(|_| ConfigError::Invalid(name))
    }

    /// A whole number in `min..=max`.
    pub fn number(&self, name: &'static str, default: u64, min: u64, max: u64) -> Result<u64, ConfigError> {
        let Some(value) = self.parse(name)? else {
            return Ok(default);
        };
        let n: u64 = value.parse().map_err(|_| ConfigError::Invalid(name))?;
        if (min..=max).contains(&n) {
            Ok(n)
        } else {
            Err(ConfigError::Invalid(name))
        }
    }

    /// A number that fits `usize`.
    pub fn size(&self, name: &'static str, default: u64, min: u64, max: u64) -> Result<usize, ConfigError> {
        usize::try_from(self.number(name, default, min, max)?).map_err(|_| ConfigError::Invalid(name))
    }

    /// A number that fits `u32`.
    pub fn count(&self, name: &'static str, default: u64, min: u64, max: u64) -> Result<u32, ConfigError> {
        u32::try_from(self.number(name, default, min, max)?).map_err(|_| ConfigError::Invalid(name))
    }

    /// A positive, finite rate.
    pub fn float(&self, name: &'static str, default: f64) -> Result<f64, ConfigError> {
        let Some(value) = self.parse(name)? else {
            return Ok(default);
        };
        let n: f64 = value.parse().map_err(|_| ConfigError::Invalid(name))?;
        if n.is_finite() && n > 0.0 && n <= 1_000_000.0 {
            Ok(n)
        } else {
            Err(ConfigError::Invalid(name))
        }
    }

    /// The name of a secret in Secret Manager (S7): the setting holds a name, never the secret.
    pub fn secret_ref(&self, name: &'static str) -> Result<Option<String>, ConfigError> {
        match self.parse(name)? {
            None => Ok(None),
            Some(value) if ports::is_valid_secret_name(&value) => Ok(Some(value)),
            Some(_) => Err(ConfigError::Invalid(name)),
        }
    }

    /// [`Reader::secret_ref`] with a default.
    pub fn secret_ref_or(&self, name: &'static str, default: &str) -> Result<String, ConfigError> {
        Ok(self.secret_ref(name)?.unwrap_or_else(|| default.to_owned()))
    }

    /// The trimmed text of a mounted file. The error names the variable that held the path, never the path or the content.
    pub fn file_text(&self, name: &'static str, path: &str) -> Result<String, ConfigError> {
        let text = (self.read_file)(Path::new(path)).map_err(|_| ConfigError::Invalid(name))?;
        let text = text.trim().to_owned();
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(ConfigError::Invalid(name));
        }
        Ok(text)
    }
}
