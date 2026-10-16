//! Checks state files declared in the instance (`data.state[].path`) for existence. Never reads
//! their contents.

use super::Importer;
use crate::ctx::Ctx;
use crate::util::{arr, s};
use crate::Doc;
use anyhow::Result;
use serde_json::{json, Map, Value};

pub struct FileState;

impl Importer for FileState {
    fn id(&self) -> &str {
        "file-state"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["data"]
    }
    fn description(&self) -> &str {
        "Existence check for declared state files (contents never read)."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        true
    }
    /// `opts.declared` is the instance's `data.state` list (filled in by the orchestrator).
    fn run(&self, ctx: &Ctx, opts: &Doc) -> Result<Doc> {
        let state: Vec<Doc> = arr(opts.get("declared"))
            .iter()
            .map(|item| {
                let mut m = item.as_object().cloned().unwrap_or_else(Map::new);
                m.insert(
                    "exists".into(),
                    Value::Bool(ctx.exists(s(item.get("path")))),
                );
                Value::Object(m)
            })
            .collect();
        Ok(json!({ "data": { "state": state } }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declared_files_get_an_exists_flag() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("notes")).unwrap();
        std::fs::write(tmp.path().join("notes/todo.md"), "- [ ] one\n").unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        assert!(FileState.detect(&ctx));
        let opts = json!({ "declared": [
            { "id": "todo", "path": "~/notes/todo.md", "kind": "markdown" },
            { "id": "journal", "path": "~/notes/journal.md", "kind": "markdown", "exists": true, "note": "kept" },
            { "id": "no-path", "kind": "dir" }
        ]});
        let out = FileState.run(&ctx, &opts).unwrap();
        assert_eq!(
            out,
            json!({ "data": { "state": [
                { "id": "todo", "path": "~/notes/todo.md", "kind": "markdown", "exists": true },
                { "id": "journal", "path": "~/notes/journal.md", "kind": "markdown", "exists": false, "note": "kept" },
                { "id": "no-path", "kind": "dir", "exists": false }
            ]}})
        );
        let keys: Vec<&str> = out["data"]["state"][1]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, ["id", "path", "kind", "exists", "note"]);
    }

    #[test]
    fn nothing_declared_means_an_empty_list() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        assert_eq!(
            FileState.run(&ctx, &json!({})).unwrap(),
            json!({ "data": { "state": [] } })
        );
    }
}
