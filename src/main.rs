//! High-performance code search engine with inverted indexing and AST-based code intelligence.
//!
//! Binary crate entry point. All CLI logic is in the `cli` module.

// Use mimalloc as global allocator — aggressively returns freed pages to the OS,
// reducing memory fragmentation by ~70-80% compared to Windows HeapAlloc.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Re-export core types from library crate
pub use code_xray::{canonicalize_or_warn, clean_path, current_unix_secs, is_inside_git_dir, is_path_within, path_eq, path_identity_key, read_file_lossy, tokenize, ContentIndex, FileEntry, FileIndex, MAX_INDEX_FILE_BYTES, Posting, TrigramIndex, DEFAULT_MIN_TOKEN_LEN, FILE_INDEX_VERSION};

// Test-only re-export: lets `crate::canonicalize_test_root` resolve from
// integration tests in this `xray` binary crate (the helper lives in the
// `code_xray` lib crate where most other test helpers live).
#[cfg(test)]
pub(crate) use code_xray::canonicalize_test_root;

mod cli;
mod definitions;
mod error;
mod git;
mod index;
mod mcp;
mod phrase_search;
mod tips;

pub use error::SearchError;
pub use index::{
    build_content_index, build_index, cleanup_indexes_for_dir, cleanup_orphaned_indexes,
    cleanup_stale_tmp_files,
    content_index_path_for, find_content_index_for_dir, index_dir, index_path_for,
    load_content_index, load_index, save_content_index, save_index,
};

// Re-export CLI types used by other modules
pub use cli::args::{IndexArgs, ContentIndexArgs, ServeArgs};

pub(crate) fn background_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A detached MCP backend has no console for child console programs to
        // inherit. Suppress window allocation without changing their stdio.
        let mut command = command;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    command
}

#[cfg(all(test, windows))]
mod background_process_tests {
    #[test]
    fn background_command_does_not_create_console() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::{Console::GetConsoleWindow, Threading::DETACHED_PROCESS};
        const TEST_NAME: &str = "background_process_tests::background_command_does_not_create_console";
        match std::env::var("XRAY_CONSOLE_TEST_STAGE").as_deref() {
            Ok("child") => {
                assert!(unsafe { GetConsoleWindow() }.is_null(), "background child allocated a console");
                println!("background-stdout-preserved");
                eprintln!("background-stderr-preserved");
            }
            Ok("detached-parent") => {
                assert!(unsafe { GetConsoleWindow() }.is_null(), "fixture must reproduce a detached backend");
                let output = super::background_command(std::env::current_exe().unwrap())
                    .args(["--exact", TEST_NAME, "--nocapture"])
                    .env("XRAY_CONSOLE_TEST_STAGE", "child")
                    .output().unwrap();
                assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
                assert!(String::from_utf8_lossy(&output.stdout).contains("background-stdout-preserved"));
                assert!(String::from_utf8_lossy(&output.stderr).contains("background-stderr-preserved"));
            }
            _ => {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", TEST_NAME, "--nocapture"])
                    .env("XRAY_CONSOLE_TEST_STAGE", "detached-parent")
                    .creation_flags(DETACHED_PROCESS)
                    .output().unwrap();
                assert!(output.status.success(), "stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            }
        }
    }
}

fn main() {
    cli::run();
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
