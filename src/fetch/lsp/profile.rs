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

pub(super) fn ensure_pyrefly_subcommand(args: &mut Vec<OsString>, profile: ServerProfile) {
    if profile == ServerProfile::Pyrefly && !args.iter().any(|arg| arg == "lsp") {
        args.insert(0, OsString::from("lsp"));
    }
}
