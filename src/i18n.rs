//! 普通用户共享的产品语言偏好与离线文案；不修改证据或机器协议。

use crate::Result;
use intl_pluralrules::{PluralCategory, PluralRuleType, PluralRules};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use unic_langid::LanguageIdentifier;

include!(concat!(env!("OUT_DIR"), "/language_catalogs.rs"));
const REGISTRY: &str = include_str!("../locales/languages.json");
static CLI_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);
static SAVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Language {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguageState {
    pub preference: String,
    pub locale: String,
    pub languages: Vec<Language>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPreference {
    preference: String,
}

pub fn languages() -> &'static [Language] {
    static LANGUAGES: OnceLock<Vec<Language>> = OnceLock::new();
    LANGUAGES.get_or_init(|| serde_json::from_str(REGISTRY).expect("语言登记格式无效"))
}

pub fn valid_preference(value: &str) -> bool {
    value == "system" || languages().iter().any(|language| language.id == value)
}

pub fn supported_locales() -> Vec<String> {
    languages()
        .iter()
        .map(|language| language.id.clone())
        .collect()
}

/// 按首选列表逐项匹配；首批所有中文变体均显示简体中文。
pub fn match_system_language(preferred: &[String], available: &[Language]) -> String {
    for preferred in preferred {
        let normalized = preferred.replace('_', "-").to_lowercase();
        if let Some(language) = available
            .iter()
            .find(|language| language.id.to_lowercase() == normalized)
        {
            return language.id.clone();
        }
        let base = normalized.split('-').next().unwrap_or("");
        if let Some(language) = available.iter().find(|language| {
            language
                .id
                .split('-')
                .next()
                .unwrap_or("")
                .eq_ignore_ascii_case(base)
        }) {
            return language.id.clone();
        }
    }
    "en".into()
}

fn system_languages() -> &'static [String] {
    static PREFERRED: OnceLock<Vec<String>> = OnceLock::new();
    PREFERRED.get_or_init(|| {
        #[cfg(target_os = "macos")]
        if let Ok(output) = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "NSGlobalDomain", "AppleLanguages"])
            .output()
            && output.status.success()
            && output.stdout.len() <= 4096
        {
            let values: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .split(['\n', ','])
                .map(|part| part.trim().trim_matches('"'))
                .filter(|part| {
                    !part.is_empty()
                        && part.chars().all(|character| {
                            character.is_ascii_alphanumeric()
                                || character == '-'
                                || character == '_'
                        })
                })
                .map(str::to_owned)
                .collect();
            if !values.is_empty() {
                return values;
            }
        }
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .map(|value| value.split('.').next().unwrap_or("en").to_owned())
            .collect()
    })
}

pub fn preference_path() -> io::Result<PathBuf> {
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    let mut buffer = vec![0u8; 65536];
    let result = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut record,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if result != 0 || found.is_null() {
        return Err(io::Error::other("language_account_unavailable"));
    }
    let home = unsafe { CStr::from_ptr(record.pw_dir) }.to_string_lossy();
    Ok(
        PathBuf::from(home.as_ref())
            .join("Library/Application Support/CodePerimeter/language.json"),
    )
}

fn read_preference(path: &Path) -> Result<String> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok("system".into()),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 4096
    {
        return Err(io::Error::other("language_file_untrusted").into());
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    let saved: SavedPreference = serde_json::from_slice(&bytes)?;
    // 已移除的语言登记按系统偏好重新解释，不永久保存一个不可用选择。
    Ok(if valid_preference(&saved.preference) {
        saved.preference
    } else {
        "system".into()
    })
}

pub fn preferred_state(path: &Path) -> Result<LanguageState> {
    let preference = read_preference(path)?;
    let locale = if preference == "system" {
        match_system_language(system_languages(), languages())
    } else {
        preference.clone()
    };
    Ok(LanguageState {
        preference,
        locale,
        languages: languages().to_vec(),
    })
}

pub fn save_preference(path: &Path, preference: &str) -> Result<LanguageState> {
    if !valid_preference(preference) {
        return Err(
            io::Error::new(io::ErrorKind::InvalidInput, "invalid_language_preference").into(),
        );
    }
    if unsafe { libc::geteuid() } == 0 {
        return Err(io::Error::other("language_requires_user").into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("language_parent_missing"))?;
    if !parent.exists() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other("language_directory_untrusted").into());
    }
    if path.exists() {
        let _ = read_preference(path)?;
    }
    let temporary = parent.join(format!(
        ".language-{}-{}.tmp",
        std::process::id(),
        SAVE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(&SavedPreference {
            preference: preference.into(),
        })?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    preferred_state(path)
}

pub fn set_cli_override(locale: Option<String>) {
    *CLI_OVERRIDE.write().expect("语言锁失效") = locale;
}

pub fn try_set_cli_override(locale: Option<String>) -> Result<()> {
    if locale
        .as_deref()
        .is_some_and(|value| !valid_preference(value))
    {
        return Err(
            io::Error::new(io::ErrorKind::InvalidInput, "invalid_language_preference").into(),
        );
    }
    set_cli_override(locale);
    Ok(())
}

pub fn current_locale() -> String {
    if let Some(preference) = CLI_OVERRIDE.read().expect("语言锁失效").as_ref() {
        return if preference == "system" {
            match_system_language(system_languages(), languages())
        } else if valid_preference(preference) {
            preference.clone()
        } else {
            "en".into()
        };
    }
    preference_path()
        .ok()
        .and_then(|path| preferred_state(&path).ok())
        .map(|state| state.locale)
        .unwrap_or_else(|| match_system_language(system_languages(), languages()))
}

fn catalogs() -> &'static BTreeMap<String, BTreeMap<String, String>> {
    static CATALOGS: OnceLock<BTreeMap<String, BTreeMap<String, String>>> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        NATIVE_CATALOGS
            .iter()
            .map(|(locale, data)| {
                (
                    (*locale).into(),
                    serde_json::from_str(data).expect("原生语言资源无效"),
                )
            })
            .collect()
    })
}

fn plural_suffix(locale: &str, count: &str) -> &'static str {
    let base = locale.split('-').next().unwrap_or("en");
    let category = base
        .parse::<LanguageIdentifier>()
        .ok()
        .and_then(|language| PluralRules::create(language, PluralRuleType::CARDINAL).ok())
        .and_then(|rules| rules.select(count).ok());
    match category {
        Some(PluralCategory::ZERO) => "zero",
        Some(PluralCategory::ONE) => "one",
        Some(PluralCategory::TWO) => "two",
        Some(PluralCategory::FEW) => "few",
        Some(PluralCategory::MANY) => "many",
        _ => "other",
    }
}

/// 替换完整句子中的命名参数，避免把插入的文件名再次当模板解释。
pub fn message(locale: &str, key: &str, args: &[(&str, String)]) -> String {
    for language in [locale, "en"] {
        let Some(catalog) = catalogs().get(language) else {
            continue;
        };
        let plural = args
            .iter()
            .find(|(name, _)| *name == "count")
            .map(|(_, count)| format!("{key}_{}", plural_suffix(language, count)));
        let Some(template) = plural
            .as_ref()
            .and_then(|key| catalog.get(key))
            .or_else(|| catalog.get(key))
        else {
            continue;
        };
        let mut output = String::new();
        let mut remaining = template.as_str();
        while let Some(start) = remaining.find("{{") {
            output.push_str(&remaining[..start]);
            let Some(end) = remaining[start + 2..].find("}}") else {
                output.push_str(&remaining[start..]);
                return output;
            };
            let name = remaining[start + 2..start + 2 + end].trim();
            if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
                output.push_str(value);
            }
            remaining = &remaining[start + 2 + end + 2..];
        }
        output.push_str(remaining);
        return output;
    }
    "Message unavailable.".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_matching_is_ordered_and_extensible() {
        assert_eq!(
            match_system_language(&["zh-TW".into(), "en-US".into()], languages()),
            "zh-CN"
        );
        assert_eq!(match_system_language(&["fr-FR".into()], languages()), "en");
        let mut registry = languages().to_vec();
        registry.push(Language {
            id: "fr".into(),
            name: "Synthetic".into(),
        });
        assert_eq!(match_system_language(&["fr-FR".into()], &registry), "fr");
    }
    #[test]
    fn saved_preference_is_private_persistent_and_validated() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private/language.json");
        assert_eq!(preferred_state(&path).unwrap().preference, "system");
        assert_eq!(save_preference(&path, "en").unwrap().locale, "en");
        assert_eq!(preferred_state(&path).unwrap().preference, "en");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert!(save_preference(&path, "invalid").is_err());
        assert_eq!(preferred_state(&path).unwrap().preference, "en");
    }
    #[test]
    fn plural_rules_cover_more_than_english() {
        assert_eq!(plural_suffix("en", "1"), "one");
        assert_eq!(plural_suffix("en", "2"), "other");
        assert_eq!(plural_suffix("zh-CN", "1"), "other");
        assert_eq!(plural_suffix("pl", "3"), "few");
    }
}
