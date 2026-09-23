//! Which language the app speaks.
//!
//! The default is whatever the system asks for; Preferences can override it.
//! Both are settled once, before GTK starts, because GTK and libadwaita
//! translate their own strings (dialog buttons, entry menus) through gettext,
//! which reads the environment at startup and never looks again. That is also
//! why changing the setting takes effect the next time Dagr runs.

use crate::settings::Language;

/// The languages Dagr ships. `en` is the fallback for anything else.
pub const SUPPORTED: [&str; 2] = ["en", "nl"];

/// The locale a setting resolves to: the override, or the first language the
/// system asks for that we actually have.
pub fn resolve(language: Language) -> &'static str {
    if let Some(locale) = language.locale() {
        return locale;
    }
    for wanted in system_languages() {
        // "nl_NL.UTF-8", "nl_NL" and macOS's "nl-NL" all mean Dutch.
        let code = wanted
            .split(['_', '-', '.', '@'])
            .next()
            .unwrap_or_default();
        if let Some(found) = SUPPORTED.iter().find(|s| **s == code) {
            return found;
        }
    }
    "en"
}

/// The languages the system asks for, most wanted first.
///
/// On macOS the environment is a poor guide: an app started from the Dock or
/// by launchd gets no `LANG` at all, and the language picked in System
/// Settings lives in `AppleLanguages` instead. Asking `defaults` for it keeps
/// the dependency list, and with it the Flatpak's vendored sources, unchanged.
fn system_languages() -> Vec<String> {
    let mut languages = Vec::new();
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
    {
        languages = parse_apple_languages(&String::from_utf8_lossy(&out.stdout));
    }
    languages.extend(gtk::glib::language_names().iter().map(|l| l.to_string()));
    languages
}

/// Reads the list `defaults read -g AppleLanguages` prints:
///
/// ```text
/// (
///     "nl-NL",
///     "en-NL"
/// )
/// ```
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_apple_languages(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.trim().trim_end_matches(',').trim_matches('"'))
        .filter(|line| !line.is_empty() && *line != "(" && *line != ")")
        .map(str::to_string)
        .collect()
}

/// Settles the language for this run. Call once, before building anything.
pub fn apply(language: Language) -> &'static str {
    let locale = resolve(language);
    // Gettext ignores `LANGUAGE` under the "C" locale, and on macOS that is
    // what an app gets whenever it starts without `LANG`, so GTK's own strings
    // would stay English. Give it a real locale, but only when nothing chose
    // one: a user's explicit `LANG` still wins.
    #[cfg(target_os = "macos")]
    if ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .all(|var| std::env::var_os(var).is_none_or(|v| v.is_empty()))
    {
        let region = match locale {
            "nl" => "nl_NL",
            _ => "en_US",
        };
        std::env::set_var("LANG", format!("{region}.UTF-8"));
    }
    // Gettext reads `LANGUAGE` for its preference list, so this is what makes
    // GTK's own strings follow an override rather than only ours. It is
    // ignored while the system locale is "C", which is correct: a C locale
    // asked for no translation at all.
    std::env::set_var("LANGUAGE", locale);
    rust_i18n::set_locale(locale);
    locale
}

/// The About dialog's blurb. `t!` resolves against the crate that declared the
/// locales, so `main.rs` cannot call it directly and asks here.
pub fn about_comments() -> String {
    crate::tr!("about.comments")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_language_list_is_read_in_order() {
        let text = "(\n    \"nl-NL\",\n    \"en-NL\"\n)\n";
        assert_eq!(parse_apple_languages(text), ["nl-NL", "en-NL"]);
        assert!(parse_apple_languages("").is_empty());
    }

    #[test]
    fn an_override_wins_over_the_system() {
        assert_eq!(resolve(Language::English), "en");
        assert_eq!(resolve(Language::Dutch), "nl");
    }

    #[test]
    fn every_string_is_translated_into_every_language() {
        // The one thing that actually rots: a string added in English and
        // forgotten in Dutch. Walk the locale file and collect, per string,
        // which languages it has.
        let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/locales/app.yml"))
            .expect("read locales/app.yml");
        let mut found: Vec<(String, Vec<&str>)> = Vec::new();
        let mut path: Vec<(usize, String)> = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = line.len() - trimmed.len();
            let Some((key, value)) = trimmed.split_once(':') else {
                continue; // a continuation line of a block of text
            };
            while path.last().is_some_and(|(at, _)| *at >= indent) {
                path.pop();
            }
            let Some(language) = SUPPORTED.iter().find(|s| **s == key) else {
                path.push((indent, key.to_string()));
                continue;
            };
            let _ = value;
            let owner = path
                .iter()
                .map(|(_, key)| key.as_str())
                .collect::<Vec<_>>()
                .join(".");
            match found.iter_mut().find(|(name, _)| *name == owner) {
                Some((_, languages)) => languages.push(language),
                None => found.push((owner, vec![language])),
            }
        }
        assert!(
            found.len() > 50,
            "only {} strings found - has the file moved?",
            found.len()
        );
        let missing: Vec<String> = found
            .iter()
            .flat_map(|(name, languages)| {
                SUPPORTED
                    .iter()
                    .filter(move |wanted| !languages.contains(wanted))
                    .map(move |wanted| format!("{name} has no {wanted}"))
            })
            .collect();
        assert!(missing.is_empty(), "untranslated: {missing:?}");
    }
}
