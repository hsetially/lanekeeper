//! Where a setting lives in a file, so findings, grid cells and search hits can link to lines.

use serde::{Deserialize, Serialize};

use crate::{LineRange, SettingPath};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingLocation {
    pub path: SettingPath,
    pub lines: LineRange,
}
