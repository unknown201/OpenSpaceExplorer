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

/// Put the table's text into every `data-loc` element (its English text kept in `data-loc-en`, for a term
/// the table lacks and for switching back).
#[cfg(target_arch = "wasm32")]
pub fn apply(doc: &web_sys::Document, loc: &std::collections::HashMap<String, String>) {
    let Ok(list) = doc.query_selector_all("[data-loc]") else {
        return;
    };
    for k in 0..list.length() {
        let Some(el) = list
            .item(k)
            .and_then(|n| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(n).ok())
        else {
            continue;
        };
        let Some(term) = el.get_attribute("data-loc") else {
            continue;
        };
        let en = match el.get_attribute("data-loc-en") {
            Some(t) => t,
            None => {
                let t = el.text_content().unwrap_or_default();
                el.set_attribute("data-loc-en", &t).ok();
                t
            }
        };
        let text = loc.get(&term).cloned().unwrap_or(en);
        if el.text_content().as_deref() != Some(text.as_str()) {
            el.set_text_content(Some(&text));
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
}
