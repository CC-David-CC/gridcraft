//! Opt-in native document ownership. The host transports bytes; this module owns the workbook,
//! pending editor, save baseline and pause lease. Transport delivery never means persistence.
use crate::SheetApp;
use gridcraft_engine::{DocState, Trust, io, model::Workbook};
use serde_json::{Value as Json, json};
use std::{cell::RefCell, sync::Arc};

pub struct Save<'a> {
    pub id: String,
    pub document: String,
    pub revision: String,
    pub name: &'a str,
    pub bytes: &'a [u8],
    pub save_as: bool,
}
pub type SaveFn = Box<dyn Fn(Save<'_>) -> Result<(), String>>;
pub type MessageFn = Box<dyn Fn(&Json) -> Result<(), String>>;
struct Pending {
    ticket: u64,
    doc: u64,
    workbook: Arc<Workbook>,
    base: Arc<Workbook>,
    path: Option<String>,
    unknown: bool,
}
#[derive(Default)]
pub struct State {
    pub enabled: bool,
    save: Option<SaveFn>,
    request: Option<MessageFn>,
    report: Option<MessageFn>,
    pending: Option<Pending>,
    ticket: u64,
    lease: Option<String>,
    observed: RefCell<(String, u64)>,
}
impl State {
    pub fn paused(&self) -> bool {
        self.lease.is_some()
    }
}
const MAX_RECOVERY: usize = 128 * 1024 * 1024;
fn path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2048
        && !value.starts_with('/')
        && !value.split('/').any(|p| p.is_empty() || p == "." || p == ".." || p.chars().any(|c| c.is_control() || c == '\\'))
}
impl SheetApp {
    pub fn enable_document_host(&mut self, save: SaveFn, request: MessageFn, report: MessageFn) -> Result<(), String> {
        if self.document_host.enabled || self.session.documents().len() > 1 {
            return Err("A document host requires a fresh single-workbook session.".into());
        }
        // Session::new() is empty, including the browser startup path. Create
        // the normal native blank before exposing hosted document operations.
        if self.session.documents().is_empty() {
            self.session.new_workbook();
        }
        self.ui.autosave = true;
        self.document_host = State { enabled: true, save: Some(save), request: Some(request), report: Some(report), ..Default::default() };
        Ok(())
    }
    /// Include transient cell/formula typing in the revision without committing it just to inspect.
    pub fn host_state(&self) -> Json {
        let fingerprint = json!([
            self.session.documents().iter().map(|d| (d.uid, d.revision, d.is_dirty())).collect::<Vec<_>>(),
            self.editor.as_ref().map(|e| format!("{e:?}")),
            self.text_box_editor.as_ref().map(|e| format!("{e:?}")),
            self.macros.editor.as_ref().map(|e| format!("{:?}", e.draft))
        ])
        .to_string();
        let mut observed = self.document_host.observed.borrow_mut();
        if observed.0 != fingerprint {
            observed.0 = fingerprint;
            observed.1 = observed.1.saturating_add(1);
        }
        let revision = observed.1.to_string();
        json!({"hostDocumentVersion":1,"hostOwnsDocuments":self.document_host.enabled,"hostDocumentState":{
            "active":self.session.active().map(|d| d.uid.to_string()),"documents":self.session.documents().iter().map(|d|json!({
                "id":d.uid.to_string(),"revision":revision,"dirty":d.is_dirty() || self.host_draft_dirty(d.uid),
                "autosaveEligible":self.host_autosave_eligible(d.uid),"autosaveEnabled":self.ui.autosave,
                "pendingSaves":usize::from(self.document_host.pending.as_ref().is_some_and(|p|p.doc==d.uid))})).collect::<Vec<_>>()}})
    }
    fn host_autosave_eligible(&self, doc: u64) -> bool {
        !self.document_host.paused()
            && self.document_host.pending.is_none()
            && self.editor.is_none()
            && self.text_box_editor.is_none()
            && self.macros.editor.is_none()
            && self.macros.question.is_none()
            && self.dialog.is_none()
            && self.backstage.is_none()
            && self.name_box.is_none()
            && self.synthetic.is_empty()
            && !self.session.in_group(doc)
    }
    fn host_draft_dirty(&self, doc: u64) -> bool {
        self.editor.as_ref().is_some_and(|e| e.doc == doc && e.original.as_deref() != Some(e.text.as_str()))
            || self.text_box_editor.is_some()
            || self.host_macro_draft()
    }
    fn host_macro_draft(&self) -> bool {
        self.macros.editor.as_ref().is_some_and(|editor| editor.has_unsaved_changes())
    }
    pub fn report_host_state(&self) {
        if let Some(report) = &self.document_host.report {
            let _ = report(&self.host_state()["hostDocumentState"]);
        }
    }
    pub(crate) fn host_command(&mut self, id: &str, params: &Json) -> Option<Result<Json, String>> {
        if matches!(id, "file.new" | "file.open" | "file.close" | "window.close" | "ui.window.close" | "ui.openFileDialog") {
            let state = self.host_state();
            let value = json!({"command":if matches!(id,"window.close"|"ui.window.close"){"file.close"}else{id},"params":params,
                "document":state["hostDocumentState"]["active"],"revision":state["hostDocumentState"]["documents"][0]["revision"]});
            return Some(
                self.document_host
                    .request
                    .as_ref()
                    .ok_or_else(|| "Document host unavailable.".into())
                    .and_then(|send| send(&value))
                    .map(|()| json!({"hostRequested":true})),
            );
        }
        if matches!(id, "view.newWindow" | "window.activate" | "view.switchWindows" | "view.hideWindow" | "view.unhideWindow") {
            return Some(Err("The host owns document windows.".into()));
        }
        if matches!(id, "file.save" | "file.saveAs") {
            return Some(self.prepare_host_save(id, params));
        }
        None
    }
    fn prepare_host_save(&mut self, operation: &str, params: &Json) -> Result<Json, String> {
        if self.document_host.pending.is_some() {
            return Err("Resolve the pending save before saving again.".into());
        }
        if params["background"].as_bool() == Some(true) {
            let doc = self.session.active().ok_or("No workbook.")?;
            if operation != "file.save" || doc.path.is_none() || params["path"].as_str().is_some_and(|p| Some(p) != doc.path.as_deref()) {
                return Err("Background Save requires the existing confirmed workbook path.".into());
            }
            if !self.ui.autosave || !self.host_autosave_eligible(doc.uid) {
                return Ok(json!({"autosave":"busy"}));
            }
        }
        if self.host_macro_draft() {
            return Err("Finish saving or discard the native macro editor's draft before saving the workbook. Its unfinished draft is not included in account saves.".into());
        }
        self.settle_drafts()?;
        self.store_view_state();
        let d = self.session.active().ok_or("No workbook.")?;
        let requested = params.get("path").and_then(Json::as_str);
        if requested.is_some_and(|p| !path(p) || !p.to_ascii_lowercase().ends_with(".xlsx")) {
            return Err("Account workbook saves require an account-relative .xlsx path.".into());
        }
        let name = requested.or(d.path.as_deref()).unwrap_or("Untitled.xlsx");
        let name = if name.to_ascii_lowercase().ends_with(".xlsx") { name.to_string() } else { format!("{name}.xlsx") };
        let bytes = io::save_bytes(&d.wb, &name).map_err(|e| e.to_string())?;
        let doc = d.uid;
        let workbook = d.wb.clone();
        let base = d.saved.clone();
        let revision = self.host_state()["hostDocumentState"]["documents"][0]["revision"].as_str().unwrap_or("0").to_string();
        self.document_host.ticket = self.document_host.ticket.checked_add(1).ok_or("Save identity exhausted.")?;
        let ticket = self.document_host.ticket;
        self.document_host.pending = Some(Pending { ticket, doc, workbook, base, path: requested.map(str::to_string), unknown: false });
        let save = Save {
            id: format!("gridcraft-save-{ticket}"),
            document: doc.to_string(),
            revision,
            name: name.rsplit('/').next().unwrap_or("Untitled.xlsx"),
            bytes: &bytes,
            save_as: operation == "file.saveAs",
        };
        if let Err(error) = self.document_host.save.as_ref().ok_or("Document save service unavailable.")?(save) {
            self.document_host.pending = None;
            return Err(error);
        }
        Ok(json!({"pendingSave":ticket}))
    }
    /// Only the embedding adapter calls this coordination surface. Ordinary commands still run
    /// through the same app/engine implementation, with an exact native target checked first.
    pub fn host_control(&mut self, request: &Json) -> Result<Json, String> {
        if !self.document_host.enabled {
            return Err("Document hosting is disabled.".into());
        }
        let command = request["command"].as_str().ok_or("Missing command.")?;
        let params = &request["params"];
        let doc = self.session.active().ok_or("No workbook.")?.uid;
        if request.get("doc").is_some_and(|v| v.as_u64() != Some(doc)) {
            return Err("Native document target changed.".into());
        }
        if command == "ui.hostPause" {
            let token = params["lease"].as_str().filter(|t| !t.is_empty() && t.len() <= 128).ok_or("Missing pause lease.")?;
            if params["paused"].as_bool() == Some(true) {
                let state = self.host_state();
                if self.document_host.paused()
                    || self.document_host.pending.is_some()
                    || params["document"] != state["hostDocumentState"]["active"]
                    || params["revision"] != state["hostDocumentState"]["documents"][0]["revision"]
                {
                    return Err("Document changed before pause.".into());
                }
                self.document_host.lease = Some(token.into());
            } else if self.document_host.lease.as_deref() == Some(token) {
                self.document_host.lease = None;
            } else {
                return Err("Pause lease does not match.".into());
            }
            return Ok(json!({"paused":self.document_host.paused()}));
        }
        if command == "document.pendingSave" {
            let pending = self
                .document_host
                .pending
                .as_ref()
                .filter(|p| Some(p.ticket) == params["ticket"].as_u64() && p.doc == doc)
                .ok_or("Save ticket does not match.")?;
            return Ok(json!({"path":pending.path,"unknown":pending.unknown}));
        }
        if command == "ui.hostSaveResult" {
            return self.finish_host_save(params, doc);
        }
        if command == "document.recoverySnapshot" {
            return self.host_recovery();
        }
        if self.document_host.paused() {
            return Err("Document is paused by its host.".into());
        }
        if command == "document.restoreSnapshot" {
            return self.restore_host_recovery(params);
        }
        if command == "file.new" {
            if self.host_state()["hostDocumentState"]["documents"][0]["dirty"] != false {
                return Err("Initial workbook has edits.".into());
            }
            return Ok(json!({"id":doc}));
        }
        if command == "file.open" {
            if self.host_state()["hostDocumentState"]["documents"][0]["dirty"] != false || self.document_host.pending.is_some() {
                return Err("Initial workbook has edits.".into());
            }
            let name = params["path"].as_str().filter(|p| path(p)).ok_or("Missing canonical account path.")?;
            if !name.to_ascii_lowercase().ends_with(".xlsx") {
                return Err("This document host currently opens .xlsx workbooks only.".into());
            }
            let data = params["data"].as_str().filter(|v| v.len() <= MAX_RECOVERY.div_ceil(3) * 4).ok_or("Missing bounded workbook bytes.")?;
            let bytes = io::base64_decode(data).ok_or("Invalid workbook encoding.")?;
            // Encrypted originals are deliberately not silently adopted into an unencrypted save.
            if bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
                return Err("Encrypted account workbooks are not supported by this host.".into());
            }
            self.session.run("file.open", json!({"name":name,"base64":data})).map_err(|e| e.to_string())?;
            let d = self.session.active_mut().ok_or("Workbook did not open.")?;
            if params["unsaved"].as_bool() != Some(true) {
                d.path = Some(name.into());
            } else {
                d.saved = Arc::new(Workbook::default());
            }
            let id = d.uid;
            if params["unsaved"].as_bool() != Some(true) {
                self.ui.recent.retain(|p| p != name);
                self.ui.recent.insert(0, name.into());
                self.ui.recent.truncate(20);
            }
            self.after_engine();
            return Ok(json!({"id":id}));
        }
        if command == "ui.backstage" {
            self.backstage = None;
            return Ok(Json::Null);
        }
        if command == "document.inspect" {
            return self.session.run(command, params.clone());
        }
        self.run(command, params.clone())
    }
    fn finish_host_save(&mut self, params: &Json, doc: u64) -> Result<Json, String> {
        let p = self
            .document_host
            .pending
            .as_mut()
            .filter(|p| p.doc == doc && Some(p.ticket) == params["ticket"].as_u64())
            .ok_or("Stale or foreign save ticket.")?;
        let outcome = params["outcome"].as_str().ok_or("Missing save outcome.")?;
        if outcome == "unknown" {
            p.unknown = true;
            return Ok(json!({"pending":true}));
        }
        if !matches!(outcome, "saved" | "failed" | "cancelled" | "conflict") {
            return Err("Unknown save outcome.".into());
        }
        if outcome == "saved" {
            let canonical =
                params["path"].as_str().filter(|p| path(p) && p.to_ascii_lowercase().ends_with(".xlsx")).ok_or("Invalid confirmed workbook path.")?;
            let d = self.session.active_mut().ok_or("Workbook was closed.")?;
            if !Arc::ptr_eq(&d.saved, &p.base) {
                return Err("The saved base changed while saving.".into());
            }
            d.saved = p.workbook.clone();
            d.path = Some(canonical.into());
            d.title = canonical.rsplit('/').next().unwrap_or("Untitled.xlsx").into();
            self.ui.recent.retain(|p| p != canonical);
            self.ui.recent.insert(0, canonical.into());
            self.ui.recent.truncate(20);
        }
        self.document_host.pending = None;
        Ok(json!({"pending":false}))
    }
    fn host_recovery(&self) -> Result<Json, String> {
        if self.text_box_editor.is_some() || self.host_macro_draft() {
            return Err("Finish the text box or macro editor before creating a recovery checkpoint.".into());
        }
        let d = self.session.active().ok_or("No workbook.")?;
        // Do not copy protected cells or hidden code into a plaintext browser journal.
        if d.wb.protected_structure || d.wb.sheets.iter().any(|s| s.is_protected()) {
            return Err("Protected workbooks require an account save; local plaintext recovery is disabled.".into());
        }
        let editor = self.editor.as_ref().map(|e| {
            json!({"sheet":e.sheet_in(&d.wb),"cell":e.cell.to_string(),"text":e.text,
            "original":e.original,"formulaBar":e.from_formula_bar,"python":e.python,"pythonCell":e.py_cell,
            "enterMode":e.enter_mode,"caret":e.caret,"anchor":e.anchor,"completion":e.completion,
            "locale":match e.locale {gridcraft_engine::formula::FormulaLocale::En=>"en",gridcraft_engine::formula::FormulaLocale::Es=>"es"}})
        });
        // Workbook's JSON intentionally omits retained package parts and sheet file IDs. Use
        // the same native XLSX codec as Save so recovery keeps those supported file features.
        let workbook = io::save_bytes(&d.wb, "recovery.xlsx").map_err(|e| e.to_string())?;
        if workbook.len() > MAX_RECOVERY {
            return Err("Workbook exceeds the recovery size limit.".into());
        }
        let data =
            serde_json::to_vec(&json!({"format":"xlsx","workbook":io::base64_encode(&workbook),"editor":editor})).map_err(|e| e.to_string())?;
        if data.len() > MAX_RECOVERY {
            return Err("Workbook exceeds the recovery size limit.".into());
        }
        Ok(json!({"version":1,"data":io::base64_encode(&data),"revision":self.host_state()["hostDocumentState"]["documents"][0]["revision"]}))
    }
    fn restore_host_recovery(&mut self, params: &Json) -> Result<Json, String> {
        if params["version"] != 1 || self.document_host.pending.is_some() {
            return Err("Unsupported recovery snapshot.".into());
        }
        let state = self.host_state();
        if params["expectedRevision"] != state["hostDocumentState"]["documents"][0]["revision"]
            || state["hostDocumentState"]["documents"][0]["dirty"] != false
        {
            return Err("Recovery target changed.".into());
        }
        let data = params["data"].as_str().filter(|d| d.len() <= MAX_RECOVERY.div_ceil(3) * 4).ok_or("Invalid recovery size.")?;
        let bytes = io::base64_decode(data).ok_or("Invalid recovery encoding.")?;
        let value: Json = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if value["format"] != "xlsx" {
            return Err("Unsupported native recovery format.".into());
        }
        let encoded = value["workbook"].as_str().filter(|v| v.len() <= MAX_RECOVERY.div_ceil(3) * 4).ok_or("Invalid recovery workbook.")?;
        let workbook_bytes = io::base64_decode(encoded).ok_or("Invalid recovery workbook encoding.")?;
        let (wb, _) = io::open_bytes_with("recovery.xlsx", &workbook_bytes, None, false).map_err(|e| e.to_string())?;
        if wb.sheets.is_empty() || wb.sheets.len() > 1024 {
            return Err("Invalid recovery workbook.".into());
        }
        let editor = &value["editor"];
        let draft = if editor.is_null() {
            None
        } else {
            let sheet = editor["sheet"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|i| *i < wb.sheets.len())
                .ok_or("Recovery editor sheet unavailable.")?;
            let cell = editor["cell"].as_str().and_then(gridcraft_engine::core::CellRef::parse).ok_or("Recovery editor cell unavailable.")?;
            let text = editor["text"].as_str().filter(|t| t.len() <= 1 << 20).ok_or("Invalid recovery cell edit.")?.to_string();
            Some((sheet, cell, text))
        };
        let source = params["sourcePath"].as_str().filter(|p| path(p)).map(str::to_string);
        let mut d = DocState::from_saved(wb, source, "Recovered workbook.xlsx".into(), Trust::FromFile);
        d.saved = Arc::new(Workbook::default());
        self.session.close_document(self.session.active_index());
        self.session.add_document(d);
        let doc = self.session.active().ok_or("Recovery failed.")?;
        let id = doc.uid;
        self.editor = draft.map(|(sheet, cell, text)| {
            let mut e = crate::editor::EditState::new(id, sheet, cell, text, true, editor["formulaBar"].as_bool() == Some(true));
            e.original = editor["original"].as_str().map(str::to_string);
            e.python = editor["python"].as_bool() == Some(true);
            e.py_cell = editor["pythonCell"].as_bool() == Some(true);
            e.enter_mode = editor["enterMode"].as_bool().unwrap_or(true);
            e.locale = match editor["locale"].as_str() {
                Some("es") => gridcraft_engine::formula::FormulaLocale::Es,
                _ => gridcraft_engine::formula::FormulaLocale::En,
            };
            let length = e.text.chars().count();
            e.caret = editor["caret"].as_u64().and_then(|v| usize::try_from(v).ok()).unwrap_or(length).min(length);
            e.anchor = editor["anchor"].as_u64().and_then(|v| usize::try_from(v).ok()).unwrap_or(e.caret).min(length);
            e.completion = editor["completion"].as_str().filter(|s| s.len() <= 1 << 20).map(str::to_string);
            e.window = self.session.active_window();
            e.on_sheet(&doc.wb)
        });
        self.session.mode = if self.editor.as_ref().is_some_and(|e| e.enter_mode) {
            gridcraft_engine::Mode::Enter
        } else if self.editor.is_some() {
            gridcraft_engine::Mode::Edit
        } else {
            gridcraft_engine::Mode::Ready
        };
        self.after_engine();
        Ok(json!({"id":id}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use std::rc::Rc;
    fn hosted() -> SheetApp {
        let mut app = SheetApp::new(gridcraft_engine::Session::new(), Services::default());
        app.enable_document_host(Box::new(|_| Ok(())), Box::new(|_| Ok(())), Box::new(|_| Ok(()))).unwrap();
        app
    }
    fn control(app: &mut SheetApp, command: &str, params: Json) -> Result<Json, String> {
        let doc = app.session.active().unwrap().uid;
        app.host_control(&json!({"command":command,"params":params,"doc":doc}))
    }
    fn ack(app: &mut SheetApp, ticket: u64, outcome: &str, path: &str) -> Result<Json, String> {
        control(app, "ui.hostSaveResult", json!({"ticket":ticket,"outcome":outcome,"path":path}))
    }
    #[test]
    fn host_initialization_creates_one_native_blank_and_never_discards_existing_workbooks() {
        let mut app = hosted();
        assert_eq!(app.session.documents().len(), 1);
        let uid = app.session.active().unwrap().uid;
        assert!(!app.session.active().unwrap().is_dirty());
        assert!(app.enable_document_host(Box::new(|_| Ok(())), Box::new(|_| Ok(())), Box::new(|_| Ok(()))).is_err());
        assert_eq!(app.session.active().unwrap().uid, uid);
        let mut session = gridcraft_engine::Session::new();
        session.new_workbook();
        session.new_workbook();
        let mut multiple = SheetApp::new(session, Services::default());
        assert!(multiple.enable_document_host(Box::new(|_| Ok(())), Box::new(|_| Ok(())), Box::new(|_| Ok(()))).is_err());
        assert_eq!(multiple.session.documents().len(), 2);
    }
    #[test]
    fn blank_closes_clean_but_explicit_save_serializes_real_xlsx() {
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let capture = bytes.clone();
        let mut app = SheetApp::new(gridcraft_engine::Session::new(), Services::default());
        app.enable_document_host(
            Box::new(move |save| {
                *capture.borrow_mut() = save.bytes.to_vec();
                Ok(())
            }),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .unwrap();
        assert_eq!(app.host_state()["hostDocumentState"]["documents"][0]["dirty"], false);
        let result = app.run("file.saveAs", json!({"path":"Documents/Blank.xlsx"})).unwrap();
        assert_eq!(result["pendingSave"], 1);
        assert!(bytes.borrow().starts_with(b"PK"));
        ack(&mut app, 1, "saved", "Documents/Blank.xlsx").unwrap();
        assert!(!app.session.active().unwrap().is_dirty());
    }
    #[test]
    fn confirmed_save_preserves_later_edits_and_native_undo() {
        let mut app = hosted();
        app.run("cell.set", json!({"cell":"A1","input":"first"})).unwrap();
        app.run("file.saveAs", json!({"path":"Documents/Book.xlsx"})).unwrap();
        assert!(app.session.active().unwrap().is_dirty(), "dispatch is not persistence");
        app.run("cell.set", json!({"cell":"A1","input":"later"})).unwrap();
        ack(&mut app, 1, "saved", "Documents/Canonical.xlsx").unwrap();
        assert!(app.session.active().unwrap().is_dirty());
        assert_eq!(app.session.active().unwrap().path.as_deref(), Some("Documents/Canonical.xlsx"));
        app.run("edit.undo", json!({})).unwrap();
        assert!(!app.session.active().unwrap().is_dirty(), "undo reaches the acknowledged snapshot");
        app.run("edit.redo", json!({})).unwrap();
        assert!(app.session.active().unwrap().is_dirty());
        assert!(ack(&mut app, 1, "saved", "Documents/Wrong.xlsx").is_err());
    }
    #[test]
    fn foreign_failed_and_unknown_results_never_rebind() {
        let mut app = hosted();
        app.run("cell.set", json!({"cell":"A1","input":"unsaved"})).unwrap();
        app.run("file.saveAs", json!({"path":"Documents/Book.xlsx"})).unwrap();
        assert!(ack(&mut app, 2, "saved", "Documents/Wrong.xlsx").is_err());
        ack(&mut app, 1, "unknown", "Documents/Book.xlsx").unwrap();
        assert!(app.run("file.save", json!({})).is_err(), "an unknown write cannot be blindly repeated");
        assert!(app.session.active().unwrap().path.is_none());
        ack(&mut app, 1, "failed", "Documents/Book.xlsx").unwrap();
        assert!(app.session.active().unwrap().is_dirty());
        assert!(app.session.active().unwrap().path.is_none());
    }
    #[test]
    fn native_save_commits_pending_formula_through_the_domain() {
        let mut app = hosted();
        app.begin_edit(Some("=SUM(1,2)".into()), true);
        app.run("file.saveAs", json!({"path":"Documents/Formula.xlsx"})).unwrap();
        assert!(app.editor.is_none());
        let pending = app.document_host.pending.as_ref().unwrap();
        let bytes = io::save_bytes(&pending.workbook, "formula.xlsx").unwrap();
        let (wb, _) = io::open_bytes_with("formula.xlsx", &bytes, None, false).unwrap();
        assert!(format!("{wb:?}").contains("SUM(1,2)"));
    }
    #[test]
    fn pause_requires_current_revision_and_matching_release() {
        let mut app = hosted();
        let state = app.host_state();
        let doc = app.session.active().unwrap().uid.to_string();
        assert!(control(&mut app, "ui.hostPause", json!({"paused":true,"lease":"one","document":doc,"revision":"stale"})).is_err());
        control(
            &mut app,
            "ui.hostPause",
            json!({"paused":true,"lease":"one","document":doc,"revision":state["hostDocumentState"]["documents"][0]["revision"]}),
        )
        .unwrap();
        assert!(app.run("cell.set", json!({"cell":"A1","input":"blocked"})).is_err());
        assert!(control(&mut app, "ui.hostPause", json!({"paused":false,"lease":"other"})).is_err());
        control(&mut app, "ui.hostPause", json!({"paused":false,"lease":"one"})).unwrap();
        app.run("cell.set", json!({"cell":"A1","input":"resumed"})).unwrap();
    }
    #[test]
    fn pending_formula_recovery_does_not_commit_or_change_live_undo() {
        let mut app = hosted();
        app.run("cell.set", json!({"cell":"A1","input":"3"})).unwrap();
        app.begin_edit(Some("=A1*7".into()), true);
        let history = app.session.active().unwrap().undo.len();
        let revision = app.host_state();
        let snapshot = control(&mut app, "document.recoverySnapshot", json!({})).unwrap();
        assert_eq!(app.editor.as_ref().unwrap().text, "=A1*7");
        assert_eq!(app.session.active().unwrap().undo.len(), history);
        assert_eq!(app.host_state(), revision);
        let mut restored = hosted();
        let before = restored.host_state();
        control(
            &mut restored,
            "document.restoreSnapshot",
            json!({"version":snapshot["version"],"data":snapshot["data"],"expectedRevision":before["hostDocumentState"]["documents"][0]["revision"]}),
        )
        .unwrap();
        assert_eq!(restored.editor.as_ref().unwrap().text, "=A1*7");
        assert!(restored.session.active().unwrap().is_dirty());
        restored.run("file.saveAs", json!({"path":"Documents/Recovered.xlsx"})).unwrap();
        assert!(restored.editor.is_none());
    }
    #[test]
    fn recovery_keeps_supported_opaque_xlsx_parts_that_native_json_omits() {
        use gridcraft_engine::model::{Preserved, PreservedPart, PreservedRel};
        let mut app = hosted();
        let original = b"<custom>retained native package content</custom>";
        let d = app.session.active_mut().unwrap();
        Arc::make_mut(&mut d.wb).preserved = Some(Arc::new(Preserved {
            rels: vec![PreservedRel {
                id: "rId91".into(),
                rel_type: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml".into(),
                target: "customXml/item1.xml".into(),
                external: false,
            }],
            parts: vec![PreservedPart {
                name: "customXml/item1.xml".into(),
                content_type: "application/xml".into(),
                data: Arc::from(original.as_slice()),
                rels: vec![],
            }],
            ..Default::default()
        }));
        let snapshot = control(&mut app, "document.recoverySnapshot", json!({})).unwrap();
        let mut restored = hosted();
        let before = restored.host_state();
        control(
            &mut restored,
            "document.restoreSnapshot",
            json!({"version":1,"data":snapshot["data"],"expectedRevision":before["hostDocumentState"]["documents"][0]["revision"]}),
        )
        .unwrap();
        let kept = restored.session.active().unwrap().wb.preserved.as_ref().unwrap();
        assert!(kept.parts.iter().any(|p| p.data.as_ref() == original));
    }
    #[test]
    fn localized_pending_formula_keeps_entry_syntax_and_caret_on_recovery() {
        let mut app = hosted();
        app.ui.language = crate::i18n::Language::Es;
        app.begin_edit(Some("=SUMA(1;2)".into()), true);
        let editor = app.editor.as_mut().unwrap();
        editor.caret = 4;
        editor.anchor = 2;
        let snapshot = control(&mut app, "document.recoverySnapshot", json!({})).unwrap();
        let mut restored = hosted();
        let before = restored.host_state();
        control(
            &mut restored,
            "document.restoreSnapshot",
            json!({"version":1,"data":snapshot["data"],"expectedRevision":before["hostDocumentState"]["documents"][0]["revision"]}),
        )
        .unwrap();
        let editor = restored.editor.as_ref().unwrap();
        assert_eq!(editor.locale, gridcraft_engine::formula::FormulaLocale::Es);
        assert_eq!((editor.caret, editor.anchor), (4, 2));
        restored.run("file.saveAs", json!({"path":"Documents/Localized.xlsx"})).unwrap();
        let cell = restored.session.run("cell.get", json!({"cell":"A1"})).unwrap();
        assert_eq!(cell["formula"], "=SUM(1,2)");
        assert_eq!(cell["value"], 3.0);
    }
    #[test]
    fn two_sheet_formulas_and_styles_survive_recovery_and_xlsx() {
        let mut app = hosted();
        app.run("sheet.rename", json!({"name":"Inputs"})).unwrap();
        app.run("cell.set", json!({"cell":"A1","input":"7"})).unwrap();
        app.run("home.bold", json!({"range":"A1","on":true})).unwrap();
        app.run("home.insertSheet", json!({"name":"Totals"})).unwrap();
        app.run("cell.set", json!({"cell":"B2","input":"=Inputs!A1*3"})).unwrap();
        let snapshot = control(&mut app, "document.recoverySnapshot", json!({})).unwrap();
        let mut restored = hosted();
        let initial = restored.host_state();
        control(
            &mut restored,
            "document.restoreSnapshot",
            json!({"version":1,"data":snapshot["data"],"expectedRevision":initial["hostDocumentState"]["documents"][0]["revision"]}),
        )
        .unwrap();
        assert_eq!(restored.session.active().unwrap().wb.sheets.len(), 2);
        let d = restored.session.active().unwrap();
        let bytes = io::save_bytes(&d.wb, "Roundtrip.xlsx").unwrap();
        let mut reopened = gridcraft_engine::Session::new();
        reopened.run("file.open", json!({"name":"Roundtrip.xlsx","base64":io::base64_encode(&bytes)})).unwrap();
        let value = reopened.run("cell.get", json!({"sheet":"Totals","cell":"B2"})).unwrap();
        assert_eq!(value["formula"], "=Inputs!A1*3");
        assert_eq!(value["value"], 21.0);
        let styled = reopened.run("cell.get", json!({"sheet":"Inputs","cell":"A1"})).unwrap();
        assert_eq!(styled["style"]["font"]["bold"], true);
    }
    #[test]
    fn draft_typing_invalidates_close_revision_and_foreign_targets_fail() {
        let mut app = hosted();
        let before = app.host_state();
        app.begin_edit(Some("unfinished".into()), true);
        let after = app.host_state();
        assert_ne!(before["hostDocumentState"]["documents"][0]["revision"], after["hostDocumentState"]["documents"][0]["revision"]);
        assert_eq!(after["hostDocumentState"]["documents"][0]["dirty"], true);
        assert!(app.host_control(&json!({"command":"cell.set","params":{"cell":"A1","input":"wrong"},"doc":u64::MAX})).is_err());
    }
    #[test]
    fn background_save_eligibility_never_finishes_pending_native_typing() {
        let mut app = hosted();
        let doc = app.session.active().unwrap().uid;
        assert!(app.host_autosave_eligible(doc));
        app.begin_edit(Some("=SUM(1,2)".into()), true);
        assert!(!app.host_autosave_eligible(doc));
        let before = app.editor.as_ref().unwrap().text.clone();
        app.session.active_mut().unwrap().path = Some("Documents/Existing.xlsx".into());
        let busy = app.run("file.save", json!({"path":"Documents/Existing.xlsx","background":true})).unwrap();
        assert_eq!(busy["autosave"], "busy");
        assert!(app.document_host.pending.is_none());
        control(&mut app, "document.recoverySnapshot", json!({})).unwrap();
        assert_eq!(app.editor.as_ref().unwrap().text, before);
        app.cancel_edit();
        assert!(app.host_autosave_eligible(doc));
        app.open_dialog("about", json!({}));
        assert!(!app.host_autosave_eligible(doc));
    }
    #[test]
    fn account_autosave_toggle_is_distinct_from_native_transient_eligibility() {
        let mut app = hosted();
        let doc = app.session.active().unwrap().uid;
        assert_eq!(app.host_state()["hostDocumentState"]["documents"][0]["autosaveEnabled"], true);
        app.session.active_mut().unwrap().path = Some("Documents/Existing.xlsx".into());
        app.toggle_autosave();
        assert_eq!(app.host_state()["hostDocumentState"]["documents"][0]["autosaveEnabled"], false);
        assert!(app.host_autosave_eligible(doc));
        assert_eq!(app.run("file.save", json!({"path":"Documents/Existing.xlsx","background":true})).unwrap()["autosave"], "busy");
        assert!(app.document_host.pending.is_none());
        app.toggle_autosave();
        assert!(app.run("file.save", json!({"path":"Documents/Existing.xlsx","background":true})).unwrap()["pendingSave"].is_number());
    }
    #[test]
    fn unfinished_native_macro_draft_is_dirty_and_requires_native_completion() {
        let mut app = hosted();
        app.open_dialog("macroEditor", json!({"new":"InitialMacro"}));
        let initial = app.host_state();
        let editor = app.macros.editor.as_mut().unwrap();
        editor.draft.name = "ChangedMacro".into();
        editor.dirty = true;
        let changed = app.host_state();
        assert_eq!(changed["hostDocumentState"]["documents"][0]["dirty"], true);
        assert_ne!(initial["hostDocumentState"]["documents"][0]["revision"], changed["hostDocumentState"]["documents"][0]["revision"]);
        assert!(app.run("file.saveAs", json!({"path":"Documents/Macro.xlsx"})).unwrap_err().contains("macro editor"));
        assert!(control(&mut app, "document.recoverySnapshot", json!({})).is_err());
        assert!(app.document_host.pending.is_none());
        let mut editor = app.macros.editor.take().unwrap();
        assert!(crate::macro_editor::save(&mut app, &mut editor));
        app.macros.editor = Some(editor);
        app.run("file.saveAs", json!({"path":"Documents/Macro.xlsx"})).unwrap();
        ack(&mut app, 1, "saved", "Documents/Macro.xlsx").unwrap();
        assert_eq!(app.host_state()["hostDocumentState"]["documents"][0]["dirty"], false);
    }
    #[test]
    fn native_file_actions_leave_the_live_buffer_in_place() {
        let mut app = hosted();
        let doc = app.session.active().unwrap().uid;
        app.run("cell.set", json!({"cell":"A1","input":"retained"})).unwrap();
        for id in ["file.new", "file.open", "file.close"] {
            assert_eq!(app.run(id, json!({})).unwrap()["hostRequested"], true);
            assert_eq!(app.session.active().unwrap().uid, doc);
        }
        assert!(app.session.active().unwrap().is_dirty());
    }
}
