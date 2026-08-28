use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result, bail};

use crate::{
    app::{App, HierarchyLoadRequest},
    config::ProjectConfig,
};

use super::{Tui, restore, resume};

pub(super) struct EditorOutcome {
    pub path: PathBuf,
    pub status: ExitStatus,
}

pub(super) struct ConfigReload {
    pub requests: Vec<HierarchyLoadRequest>,
    pub filters: crate::config::FilterConfig,
}

pub(super) fn open(workspace: &Path) -> Result<EditorOutcome> {
    let editor = select_editor(std::env::var_os("EDITER"), std::env::var_os("EDITOR"))?;
    open_with_editor(&editor, workspace)
}

pub(super) fn edit_project_config(
    terminal: &mut Tui,
    app: &mut App,
    hierarchy_available: bool,
) -> Result<Option<ConfigReload>> {
    restore(terminal)?;
    let edit_result = open(&app.workspace);
    resume(terminal)?;

    let outcome = match edit_result {
        Ok(outcome) => outcome,
        Err(error) => {
            app.set_canvas_error(format!("Project config editor failed: {error:#}"));
            return Ok(None);
        }
    };
    if !outcome.status.success() {
        app.set_canvas_error(format!(
            "Project config editor exited with {}",
            outcome.status
        ));
        return Ok(None);
    }
    let config = match ProjectConfig::load(&app.workspace) {
        Ok(config) => config,
        Err(error) => {
            app.set_canvas_error(format!(
                "Project config reload failed for {}: {error:#}",
                outcome.path.display()
            ));
            return Ok(None);
        }
    };
    let filters = config.filters.clone();
    let requests = app.reload_filter_config(filters.clone(), hierarchy_available);
    Ok(Some(ConfigReload { requests, filters }))
}

fn open_with_editor(editor: &OsStr, workspace: &Path) -> Result<EditorOutcome> {
    let path = ProjectConfig::create_if_missing(workspace)?;
    let status = Command::new(editor)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to start editor {:?}", editor))?;
    Ok(EditorOutcome { path, status })
}

fn select_editor(editer: Option<OsString>, editor: Option<OsString>) -> Result<OsString> {
    let program = editer.or(editor).filter(|program| !program.is_empty());
    let Some(program) = program else {
        bail!("set $EDITER or $EDITOR to edit the project config");
    };
    Ok(program)
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{open_with_editor, select_editor};

    #[test]
    fn prefers_editer_and_falls_back_to_standard_editor() {
        assert_eq!(
            select_editor(Some(OsString::from("nvim")), Some(OsString::from("vim"))).unwrap(),
            OsStr::new("nvim")
        );
        assert_eq!(
            select_editor(None, Some(OsString::from("vim"))).unwrap(),
            OsStr::new("vim")
        );
        assert!(
            select_editor(None, None)
                .unwrap_err()
                .to_string()
                .contains("$EDITER")
        );
    }

    #[test]
    fn passes_the_project_config_path_to_the_editor_process() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!("cgraph-editor-{unique}"));
        fs::create_dir(&workspace).unwrap();
        let editor = workspace.join("record-editor");
        fs::write(&editor, "#!/bin/sh\nprintf '%s' \"$1\" > \"$1.seen\"\n").unwrap();
        fs::set_permissions(&editor, fs::Permissions::from_mode(0o700)).unwrap();

        let outcome = open_with_editor(editor.as_os_str(), &workspace).unwrap();

        assert!(outcome.status.success());
        assert_eq!(outcome.path, workspace.join(".cgraph.toml"));
        assert_eq!(
            fs::read_to_string(workspace.join(".cgraph.toml.seen")).unwrap(),
            outcome.path.display().to_string()
        );
        assert!(
            fs::read_to_string(&outcome.path)
                .unwrap()
                .contains("rules = []")
        );
        fs::remove_dir_all(workspace).unwrap();
    }
}
