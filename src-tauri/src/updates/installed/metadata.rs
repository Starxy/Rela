//! The complete, fixed set of system metadata changed by Rela's perMachine
//! NSIS template. Serialized records cannot select registry keys or file paths.
use super::pending;
use base64::{engine::general_purpose::STANDARD, Engine};
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    InstallRoot,
    MainBinaryName,
    DisplayName,
    DisplayIcon,
    DisplayVersion,
    Publisher,
    InstallLocation,
    UninstallString,
    NoModify,
    NoRepair,
    EstimatedSize,
    UrlInfoAbout,
    UrlUpdateInfo,
    HelpLink,
}
impl Field {
    pub const ALL: [Self; 14] = [
        Self::InstallRoot,
        Self::MainBinaryName,
        Self::DisplayName,
        Self::DisplayIcon,
        Self::DisplayVersion,
        Self::Publisher,
        Self::InstallLocation,
        Self::UninstallString,
        Self::NoModify,
        Self::NoRepair,
        Self::EstimatedSize,
        Self::UrlInfoAbout,
        Self::UrlUpdateInfo,
        Self::HelpLink,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::InstallRoot => "",
            Self::MainBinaryName => "MainBinaryName",
            Self::DisplayName => "DisplayName",
            Self::DisplayIcon => "DisplayIcon",
            Self::DisplayVersion => "DisplayVersion",
            Self::Publisher => "Publisher",
            Self::InstallLocation => "InstallLocation",
            Self::UninstallString => "UninstallString",
            Self::NoModify => "NoModify",
            Self::NoRepair => "NoRepair",
            Self::EstimatedSize => "EstimatedSize",
            Self::UrlInfoAbout => "URLInfoAbout",
            Self::UrlUpdateInfo => "URLUpdateInfo",
            Self::HelpLink => "HelpLink",
        }
    }
    fn is_dword(self) -> bool {
        matches!(self, Self::NoModify | Self::NoRepair | Self::EstimatedSize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shortcut {
    StartMenu,
    Desktop,
}
impl Shortcut {
    pub const ALL: [Self; 2] = [Self::StartMenu, Self::Desktop];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Value {
    String(String),
    Dword(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub registry: BTreeMap<Field, Option<Value>>,
    /// Raw shortcut bytes in Base64; no target path comes from this record.
    pub shortcuts: BTreeMap<Shortcut, Option<String>>,
}

impl Snapshot {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.registry.keys().copied().collect::<Vec<_>>() != Field::ALL
            || self.shortcuts.keys().copied().collect::<Vec<_>>() != Shortcut::ALL
        {
            return Err(pending());
        }
        for (field, value) in &self.registry {
            match value {
                Some(Value::String(value))
                    if !field.is_dword()
                        && value.encode_utf16().count() <= 2047
                        && !value.contains('\0') => {}
                Some(Value::Dword(_)) if field.is_dword() => {}
                None => {}
                _ => return Err(pending()),
            }
        }
        for value in self.shortcuts.values().flatten() {
            if value.len() > 350_000
                || STANDARD.decode(value).map_err(|_| pending())?.len() > 256 * 1024
            {
                return Err(pending());
            }
        }
        Ok(())
    }

    pub fn validate_installation(&self, root: &Path, version: &Version) -> Result<(), AppError> {
        self.validate()?;
        let root = root.to_str().ok_or_else(pending)?;
        let directory = normalized(root);
        let string = |field: Field| match self.registry.get(&field) {
            Some(Some(Value::String(value))) => Ok(value.as_str()),
            _ => Err(pending()),
        };
        let quoted = |value: &str| -> Result<String, AppError> {
            value
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .map(normalized)
                .ok_or_else(pending)
        };
        if normalized(string(Field::InstallRoot)?) != directory
            || quoted(string(Field::InstallLocation)?)? != directory
            // The supported updater does not rename the primary binary; a
            // legacy name requires a manual first upgrade, before auto updates.
            || string(Field::MainBinaryName)? != "rela.exe"
            || string(Field::DisplayName)? != "Rela"
            || string(Field::Publisher)? != "teamsillybees"
            || Version::parse(string(Field::DisplayVersion)?).map_err(|_| pending())? != *version
            || quoted(string(Field::DisplayIcon)?)? != format!("{directory}\\rela.exe")
            || quoted(string(Field::UninstallString)?)? != format!("{directory}\\uninstall.exe")
            || self.registry[&Field::NoModify] != Some(Value::Dword(1))
            || self.registry[&Field::NoRepair] != Some(Value::Dword(1))
            || !matches!(self.registry[&Field::EstimatedSize], Some(Value::Dword(_)))
        {
            return Err(pending());
        }
        Ok(())
    }
}

fn normalized(value: &str) -> String {
    value
        .strip_prefix("\\\\?\\")
        .unwrap_or(value)
        .trim_end_matches('\\')
        .to_lowercase()
}
