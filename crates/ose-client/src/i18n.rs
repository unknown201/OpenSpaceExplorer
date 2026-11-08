//! The game's languages (T45): KSP2's I2 string tables, exported per language by
//! `tools/export_localization.py` to `assets/loc/<code>.json`, picked in Settings > Interface. The page's own
//! text that KSP2 words the same way carries its term (`data-loc`); game content (part names and
//! descriptions, the tech tree, missions, loading tips) already reads the table.

/// The languages KSP2 ships, with the codes the exporter writes and their own names for the picker.
pub const LANGUAGES: [(&str, &str); 12] = [
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("pl", "Polski"),
    ("pt", "Português (Brasil)"),
    ("ru", "Русский"),
    ("zh", "简体中文"),
    ("zh-tw", "繁體中文"),
];

/// The language to use: the player's choice, else the browser's when the game has it, else English.
pub fn pick(stored: Option<&str>, browser: Option<&str>) -> &'static str {
    let known = |c: &str| LANGUAGES.iter().map(|l| l.0).find(|&k| k == c);
    if let Some(k) = stored.and_then(known) {
        return k;
    }
    if let Some(b) = browser.map(|b| b.to_ascii_lowercase()) {
        // zh-TW and zh-HK read traditional characters; any other zh simplified.
        if b.starts_with("zh-tw") || b.starts_with("zh-hk") || b.starts_with("zh-hant") {
            return "zh-tw";
        }
        if let Some(k) = known(b.split('-').next().unwrap_or("")) {
            return k;
        }
    }
    "en"
}

/// The table to use: `lang` over English, so a term the language lacks still reads.
pub fn merge(
    mut en: std::collections::HashMap<String, String>,
    lang: Option<std::collections::HashMap<String, String>>,
) -> std::collections::HashMap<String, String> {
    if let Some(l) = lang {
        en.extend(l.into_iter().filter(|(_, v)| !v.trim().is_empty()));
    }
    en
}

type Table = std::rc::Rc<std::collections::HashMap<String, String>>;

thread_local! {
    /// The table in use outside English, for text the HUD's free functions write (`t`); `None` in English.
    static ACTIVE: std::cell::RefCell<Option<Table>> = const { std::cell::RefCell::new(None) };
}

/// Make `table` the one `t` reads, or none for English.
pub fn set_active(table: Option<Table>) {
    ACTIVE.with(|a| *a.borrow_mut() = table);
}

/// Text OSE writes itself, in the player's language: the game's `term`, or OSE's own English (`own`).
pub fn t(term: &str, own: &str) -> String {
    ACTIVE.with(|a| match a.borrow().as_ref() {
        Some(loc) => pick_text(loc, term, own, false).to_string(),
        None => own.to_string(),
    })
}

/// Where a term goes: `data-loc` fills the text, `data-loc-title` the tooltip and `data-loc-cap` the `data-cap`
/// attribute that a CSS caption shows (`content: attr(data-cap)`).
pub const SLOTS: [(&str, Option<&str>); 3] = [
    ("data-loc", None),
    ("data-loc-title", Some("title")),
    ("data-loc-cap", Some("data-cap")),
];

/// The text an element shows for `term`: OSE's own English wording (`own`) in English, else the table's, else
/// that English again.
pub fn pick_text<'a>(
    loc: &'a std::collections::HashMap<String, String>,
    term: &str,
    own: &'a str,
    english: bool,
) -> &'a str {
    if english {
        return own;
    }
    loc.get(term).map(String::as_str).unwrap_or(own)
}

/// Put the table's text into every element carrying a term (`SLOTS`), keeping its English in
/// `<slot>-en` for a term the table lacks and for switching back. In English the page keeps OSE's own
/// wording, which for tooltips is often longer than the game's short name.
#[cfg(target_arch = "wasm32")]
pub fn apply(
    doc: &web_sys::Document,
    loc: &std::collections::HashMap<String, String>,
    english: bool,
) {
    for (slot, attr) in SLOTS {
        let Ok(list) = doc.query_selector_all(&format!("[{slot}]")) else {
            continue;
        };
        let keep = format!("{slot}-en");
        for k in 0..list.length() {
            let Some(el) = list
                .item(k)
                .and_then(|n| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(n).ok())
            else {
                continue;
            };
            let Some(term) = el.get_attribute(slot) else {
                continue;
            };
            let current = match attr {
                None => el.text_content(),
                Some(a) => el.get_attribute(a),
            }
            .unwrap_or_default();
            let own = match el.get_attribute(&keep) {
                Some(t) => t,
                None => {
                    el.set_attribute(&keep, &current).ok();
                    current.clone()
                }
            };
            let text = pick_text(loc, &term, &own, english);
            if current != text {
                match attr {
                    None => el.set_text_content(Some(text)),
                    Some(a) => {
                        el.set_attribute(a, text).ok();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_players_choice_then_the_browsers_then_english() {
        assert_eq!(pick(Some("de"), Some("fr-FR")), "de");
        assert_eq!(pick(None, Some("fr-FR")), "fr");
        assert_eq!(pick(None, Some("zh-TW")), "zh-tw");
        assert_eq!(pick(None, Some("zh-CN")), "zh");
        assert_eq!(pick(Some("xx"), Some("nl-NL")), "en");
        assert_eq!(pick(None, None), "en");
    }

    #[test]
    fn a_language_falls_back_to_english_term_by_term() {
        let en = [("a", "Launch"), ("b", "Back")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let de = [("a", "Starten"), ("b", " ")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let m = merge(en, Some(de));
        assert_eq!(m["a"], "Starten");
        assert_eq!(m["b"], "Back");
    }

    #[test]
    fn english_keeps_oses_own_wording_and_other_languages_take_the_games() {
        let loc = [("HUD/Tooltips/Prograde", "Rechtläufig")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let own = "Prograde: point along the velocity";
        assert_eq!(pick_text(&loc, "HUD/Tooltips/Prograde", own, true), own);
        assert_eq!(
            pick_text(&loc, "HUD/Tooltips/Prograde", own, false),
            "Rechtläufig"
        );
        assert_eq!(pick_text(&loc, "HUD/Missing", own, false), own);
    }

    #[test]
    fn hud_text_follows_the_active_table() {
        assert_eq!(t("Flight/ResourceManager/Out", "Out"), "Out");
        let de: std::collections::HashMap<String, String> =
            [("Flight/ResourceManager/Out".to_string(), "Aus".to_string())].into();
        set_active(Some(std::rc::Rc::new(de)));
        assert_eq!(t("Flight/ResourceManager/Out", "Out"), "Aus");
        set_active(None);
        assert_eq!(t("Flight/ResourceManager/Out", "Out"), "Out");
    }
}
