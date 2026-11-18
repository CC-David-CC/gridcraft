//! Preferences between visits (#1562): what the desktop app keeps in `ui.json` (theme, interface
//! size, language, ribbon state…) and `prefs.json` (editing options, user name…), kept in the
//! browser's `localStorage` under [`UI_KEY`] and [`PREFS_KEY`]. Restored at startup; written back
//! whenever they change, since a reload or a closed tab runs no exit code.

use gridcraft_ui_egui::SheetApp;

/// The `localStorage` key for the desktop's `ui.json`.
pub const UI_KEY: &str = "gridcraft.ui";
/// The `localStorage` key for the desktop's `prefs.json`.
pub const PREFS_KEY: &str = "gridcraft.prefs";
/// Largest stored value read back (`localStorage` holds about 5 MB per site).
const MAX_STORED: usize = 1 << 20;

/// Apply preferences stored on an earlier visit, as the desktop app reads `ui.json` and
/// `prefs.json`. Missing, oversized or unreadable values leave what the app starts with.
pub fn restore(app: &mut SheetApp, ui: Option<&str>, prefs: Option<&str>) {
    let readable = |s: &&str| s.len() <= MAX_STORED;
    if let Some(s) = ui.filter(readable)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(s)
        && let Ok(mut state) = serde_json::from_value::<gridcraft_ui_egui::UiState>(json.clone())
    {
        // No usable saved language: keep the browser's, as the desktop keeps the system's.
        let language = gridcraft_ui_egui::i18n::saved_language(&json).unwrap_or(app.ui.language);
        // A hosted profile's absent choice follows account AutoSave policy. An explicit
        // account-scoped Off survives; standalone defaults remain unchanged.
        if app.document_host.enabled && json.get("autosave").and_then(serde_json::Value::as_bool).is_none() {
            state.autosave = true;
        }
        app.ui = state.at_startup();
        app.ui.language = language;
    }
    if let Some(s) = prefs.filter(readable)
        && let Ok(p) = serde_json::from_str(s)
    {
        app.session.prefs = p;
    }
}

/// Tracks what was last written, so the store is written only when a preference changed.
#[derive(Default)]
pub struct PrefsSync {
    ui: Option<String>,
    prefs: Option<String>,
    /// The user name to store in place of the session's (see [`Self::keep_user_name`]).
    user_name: Option<String>,
}

impl PrefsSync {
    /// Starts from what is already stored, so restoring doesn't write it straight back.
    pub fn new(ui: Option<String>, prefs: Option<String>) -> Self {
        Self { ui, prefs, user_name: None }
    }

    /// Stores `name` as the user name whatever the session's becomes: under `?host=parent` the
    /// framing page names the author for this visit only, and must not replace the user's own.
    pub fn keep_user_name(&mut self, name: String) {
        self.user_name = Some(name);
    }

    /// The `(key, JSON)` pairs to store because they differ from what was last written.
    pub fn changed(&mut self, app: &SheetApp) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let prefs = match &self.user_name {
            Some(name) => serde_json::to_string(&gridcraft_engine::Prefs { user_name: name.clone(), ..app.session.prefs.clone() }),
            None => serde_json::to_string(&app.session.prefs),
        };
        for (key, last, json) in [(UI_KEY, &mut self.ui, serde_json::to_string(&app.ui)), (PREFS_KEY, &mut self.prefs, prefs)] {
            match json {
                Ok(json) if last.as_deref() != Some(json.as_str()) => {
                    *last = Some(json.clone());
                    out.push((key, json));
                }
                Ok(_) => {}
                Err(e) => log::warn!("preferences: cannot serialize {key}: {e}"),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gridcraft_engine::Session;
    use gridcraft_ui_egui::Services;
    use gridcraft_ui_egui::i18n::Language;
    use serde_json::json;

    fn app() -> SheetApp {
        let mut app = SheetApp::new(Session::new(), Services::default());
        // As `web::start` sets it up before restoring.
        app.ui.system_theme = true;
        app.ui.language = Language::De;
        app
    }

    fn stored(writes: &[(&'static str, String)], key: &str) -> Option<String> {
        writes.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
    }

    /// #1562: dark mode chosen on one visit is there after a reload, with the editing options.
    #[test]
    fn dark_mode_survives_reload() {
        let mut first = app();
        let mut sync = PrefsSync::new(None, None);
        assert_eq!(sync.changed(&first).len(), 2, "first frame stores the preferences");
        assert!(sync.changed(&first).is_empty(), "unchanged preferences are not rewritten");
        first.run("view.theme", json!({"mode": "dark"})).unwrap();
        first.session.prefs.enter_direction = "right".into();
        let writes = sync.changed(&first);
        let (ui, prefs) = (stored(&writes, UI_KEY), stored(&writes, PREFS_KEY));
        assert!(ui.is_some() && prefs.is_some(), "both changes are stored: {writes:?}");

        let mut reloaded = app();
        restore(&mut reloaded, ui.as_deref(), prefs.as_deref());
        assert!(reloaded.ui.dark && !reloaded.ui.system_theme, "dark mode is back");
        assert_eq!(reloaded.session.prefs.enter_direction, "right");
        assert!(PrefsSync::new(ui, prefs).changed(&reloaded).is_empty(), "restoring doesn't write back");
    }

    /// `?host=parent`: the author the framing page names is not stored over the user's own name.
    #[test]
    fn host_author_is_not_stored() {
        let mut first = app();
        first.session.prefs.user_name = "Ada".into();
        let mut sync = PrefsSync::new(None, None);
        let stored_prefs = stored(&sync.changed(&first), PREFS_KEY);

        let mut hosted = app();
        restore(&mut hosted, None, stored_prefs.as_deref());
        let mut sync = PrefsSync::new(None, stored_prefs);
        sync.keep_user_name(hosted.session.prefs.user_name.clone());
        hosted.session.prefs.user_name = "Host Author".into();
        hosted.session.prefs.r1c1 = true;
        let writes = sync.changed(&hosted);
        let prefs = stored(&writes, PREFS_KEY).expect("other preferences are still stored");

        let mut reloaded = app();
        restore(&mut reloaded, None, Some(&prefs));
        assert_eq!(reloaded.session.prefs.user_name, "Ada");
        assert!(reloaded.session.prefs.r1c1);
    }

    /// A stored `ui` without a language keeps the browser's, as the desktop keeps the system's.
    #[test]
    fn missing_language_keeps_the_browsers() {
        let mut a = app();
        restore(&mut a, Some(r#"{"dark":true}"#), None);
        assert!(a.ui.dark);
        assert_eq!(a.ui.language, Language::De);
        restore(&mut a, Some(r#"{"language":"fr"}"#), None);
        assert_eq!(a.ui.language, Language::Fr);
    }

    #[test]
    fn junk_storage_keeps_defaults() {
        let big = format!("{{\"dark\":true,\"recent\":[\"{}\"]}}", "x".repeat(MAX_STORED));
        for junk in [None, Some(""), Some("not json"), Some("[1,2]"), Some("{\"dark\":42}"), Some(big.as_str())] {
            let mut a = app();
            restore(&mut a, junk, junk);
            assert!(a.ui.system_theme && !a.ui.dark, "{junk:?}");
            assert_eq!(a.ui.language, Language::De);
            assert_eq!(a.session.prefs.enter_direction, "down");
        }
    }
}

/// Hosted preferences never read the origin-global standalone profile.
pub struct Profile {
    hosted: bool,
    namespace: Option<String>,
}
impl Profile {
    pub fn new(hosted: bool, namespace: Option<&str>) -> Self {
        Self {
            hosted,
            namespace: namespace
                .filter(|s| !s.is_empty() && s.len() <= 256 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_:-".contains(&b)))
                .map(str::to_string),
        }
    }
    pub fn key(&self, key: &str) -> Option<String> {
        if !self.hosted { Some(key.into()) } else { self.namespace.as_ref().map(|id| format!("{key}.account.{id}")) }
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    #[test]
    fn hosted_autosave_defaults_on_and_respects_explicit_account_choice() {
        fn hosted() -> SheetApp {
            let mut app = SheetApp::new(gridcraft_engine::Session::new(), gridcraft_ui_egui::Services::default());
            app.enable_document_host(Box::new(|_| Ok(())), Box::new(|_| Ok(())), Box::new(|_| Ok(()))).unwrap();
            app
        }
        let mut app = hosted();
        restore(&mut app, None, None);
        assert!(app.ui.autosave);
        restore(&mut app, Some(r#"{"dark":true}"#), None);
        assert!(app.ui.autosave);
        restore(&mut app, Some(r#"{"autosave":false}"#), None);
        assert!(!app.ui.autosave);
        let saved = serde_json::to_string(&app.ui).unwrap();
        let mut next = hosted();
        restore(&mut next, Some(&saved), None);
        assert!(!next.ui.autosave);
        let standalone = SheetApp::new(gridcraft_engine::Session::new(), gridcraft_ui_egui::Services::default());
        assert!(!standalone.ui.autosave);
    }
    #[test]
    fn hosted_profiles_never_fall_back_to_origin_global_preferences() {
        for profile in [None, Some(""), Some("../account"), Some("account with spaces")] {
            assert_eq!(Profile::new(true, profile).key(UI_KEY), None);
        }
        let a = Profile::new(true, Some("issuer_account_A"));
        let b = Profile::new(true, Some("issuer_account_B"));
        assert_ne!(a.key(UI_KEY), b.key(UI_KEY));
        assert_ne!(a.key(UI_KEY), Some(UI_KEY.into()));
        assert_eq!(Profile::new(false, None).key(UI_KEY), Some(UI_KEY.into()));
    }
}
