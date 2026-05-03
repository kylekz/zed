use claude_code_ide_server::{Position, SelectionPayload, SelectionRange};
use std::path::Path;

/// Builds the `SelectionPayload` JSON shape sent to the `claude` CLI from
/// raw selection data. Mirrors the VS Code extension contract.
pub fn build(
    text: String,
    abs_path: &Path,
    start_line: u32,
    start_character: u32,
    end_line: u32,
    end_character: u32,
) -> SelectionPayload {
    let file_path = abs_path.to_string_lossy().into_owned();
    let file_url = file_url(abs_path);
    let is_empty =
        start_line == end_line && start_character == end_character;
    SelectionPayload {
        text,
        file_path,
        file_url,
        selection: SelectionRange {
            start: Position {
                line: start_line,
                character: start_character,
            },
            end: Position {
                line: end_line,
                character: end_character,
            },
            is_empty,
        },
    }
}

fn file_url(abs_path: &Path) -> String {
    url::Url::from_file_path(abs_path)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| format!("file://{}", abs_path.display()))
}
