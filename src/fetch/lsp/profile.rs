use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ServerProfile {
    Standard,
    RustAnalyzer,
    Clangd,
    Pyrefly,
}

pub(super) fn from_name(name: &str) -> ServerProfile {
    let normalized = Path::new(name)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(name)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    match normalized.as_str() {
        "rust-analyzer" | "rust_analyzer" => ServerProfile::RustAnalyzer,
        "clangd" => ServerProfile::Clangd,
        "pyrefly" | "pyrefly-lsp" | "pyrefly_lsp" => ServerProfile::Pyrefly,
        _ => ServerProfile::Standard,
    }
}

pub(super) fn from_program(program: &OsStr) -> ServerProfile {
    from_name(&Path::new(program).to_string_lossy())
}

pub(super) fn file_extensions(server_name: &str) -> &'static [&'static str] {
    match from_name(server_name) {
        ServerProfile::RustAnalyzer => &["rs"],
        ServerProfile::Clangd => &["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"],
        ServerProfile::Pyrefly => &["py", "pyi"],
        ServerProfile::Standard => &[],
    }
}

pub(super) fn apply_default_args(args: &mut Vec<OsString>, profile: ServerProfile) {
    match profile {
        ServerProfile::Pyrefly if !args.iter().any(|arg| arg == "lsp") => {
            args.insert(0, OsString::from("lsp"));
        }
        ServerProfile::Clangd if !has_background_index_setting(args) => {
            args.push(OsString::from("--background-index"));
        }
        _ => {}
    }
}

pub(super) fn append_configured_args(
    args: &mut Vec<OsString>,
    configured: impl IntoIterator<Item = OsString>,
    profile: ServerProfile,
) {
    let configured = configured.into_iter().collect::<Vec<_>>();
    if profile == ServerProfile::Clangd && has_background_index_setting(&configured) {
        args.retain(|arg| arg != "--background-index");
    }
    args.extend(configured);
    apply_default_args(args, profile);
}

fn has_background_index_setting(args: &[OsString]) -> bool {
    args.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        arg == "--background-index"
            || arg == "--no-background-index"
            || arg.starts_with("--background-index=")
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{ServerProfile, append_configured_args, apply_default_args};

    #[test]
    fn enables_clangd_background_index_by_default() {
        let mut args = Vec::new();
        apply_default_args(&mut args, ServerProfile::Clangd);
        apply_default_args(&mut args, ServerProfile::Clangd);

        assert_eq!(args, [OsString::from("--background-index")]);
    }

    #[test]
    fn preserves_explicit_clangd_background_index_policy() {
        let mut args = vec![OsString::from("--background-index")];
        append_configured_args(
            &mut args,
            [OsString::from("--no-background-index")],
            ServerProfile::Clangd,
        );

        assert_eq!(args, [OsString::from("--no-background-index")]);
    }
}
