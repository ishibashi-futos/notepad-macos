use std::path::{Path, PathBuf};

use encoding_rs::{Encoding, SHIFT_JIS, UTF_16BE, UTF_16LE, UTF_8};
use ropey::Rope;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    ShiftJis,
}

impl TextEncoding {
    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf16Le => "UTF-16LE",
            TextEncoding::Utf16Be => "UTF-16BE",
            TextEncoding::ShiftJis => "Shift_JIS",
        }
    }

    pub fn encoding(self) -> &'static Encoding {
        match self {
            TextEncoding::Utf8 => UTF_8,
            TextEncoding::Utf16Le => UTF_16LE,
            TextEncoding::Utf16Be => UTF_16BE,
            TextEncoding::ShiftJis => SHIFT_JIS,
        }
    }

    pub fn bom(self) -> &'static [u8] {
        match self {
            TextEncoding::Utf8 => &[],
            TextEncoding::Utf16Le => &[0xFF, 0xFE],
            TextEncoding::Utf16Be => &[0xFE, 0xFF],
            TextEncoding::ShiftJis => &[],
        }
    }

    pub fn from_encoding(encoding: &'static Encoding) -> Option<Self> {
        if encoding == UTF_8 {
            Some(TextEncoding::Utf8)
        } else if encoding == UTF_16LE {
            Some(TextEncoding::Utf16Le)
        } else if encoding == UTF_16BE {
            Some(TextEncoding::Utf16Be)
        } else if encoding == SHIFT_JIS {
            Some(TextEncoding::ShiftJis)
        } else {
            None
        }
    }

    pub fn next(self) -> Self {
        match self {
            TextEncoding::Utf8 => TextEncoding::Utf16Le,
            TextEncoding::Utf16Le => TextEncoding::Utf16Be,
            TextEncoding::Utf16Be => TextEncoding::ShiftJis,
            TextEncoding::ShiftJis => TextEncoding::Utf8,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum CoreError {
    System(SystemError),
    Domain(DomainError),
}

#[derive(Debug)]
pub struct SystemError {
    pub kind: SystemErrorKind,
    pub context: String,
    pub retriable: bool,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum SystemErrorKind {
    Io,
    Permission,
    Encoding,
    Os,
    Unknown,
}

#[derive(Debug)]
pub struct DomainError {
    pub kind: DomainErrorKind,
    pub context: String,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum DomainErrorKind {
    InvalidOperation,
    InvalidState,
    OutOfRange,
    EmptySelection,
}

#[derive(Debug, Clone)]
struct Preedit {
    text: String,
    cursor: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct Edit {
    kind: EditKind,
    cursor_before: usize,
    cursor_after: usize,
}

#[derive(Debug, Clone)]
enum EditKind {
    Insert { idx: usize, text: String },
    Delete { idx: usize, text: String },
    Replace {
        idx: usize,
        deleted: String,
        inserted: String,
    },
}

pub struct Core {
    rope: Rope,
    cursor: usize,
    selection_anchor: Option<usize>,
    preedit: Option<Preedit>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    path: Option<PathBuf>,
    encoding: TextEncoding,
    dirty: bool,
}

impl Core {
    const PLACEHOLDER_TEXT: &'static str = "Type here...";
    const UNDO_LIMIT: usize = 100;

    pub fn new() -> Self {
        Self {
            rope: Rope::from_str(""),
            cursor: 0,
            selection_anchor: None,
            preedit: None,
            undo: Vec::new(),
            redo: Vec::new(),
            path: None,
            encoding: TextEncoding::Utf8,
            dirty: false,
        }
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line_len_chars(&self, line: usize) -> usize {
        line_len_chars(&self.rope, line)
    }

    pub fn set_cursor_line_col(&mut self, line: usize, col: usize, extend: bool) -> bool {
        if self.rope.len_chars() == 0 {
            let before = self.cursor;
            self.set_cursor(0, extend);
            return self.cursor != before;
        }
        let max_line = self.rope.len_lines().saturating_sub(1);
        let line = line.min(max_line);
        let line_len = line_len_chars(&self.rope, line);
        let target_col = col.min(line_len);
        let next = self.rope.line_to_char(line) + target_col;
        let before = self.cursor;
        let before_selection = self.selection_range();
        self.set_cursor(next, extend);
        self.cursor != before || self.selection_range() != before_selection
    }

    pub fn display_text(&self) -> String {
        if let Some(preedit) = &self.preedit {
            let mut text = self.rope.to_string();
            let insert_at = char_to_byte_idx(&text, self.cursor);
            text.insert_str(insert_at, &preedit.text);
            text
        } else if self.rope.len_chars() == 0 {
            Self::PLACEHOLDER_TEXT.to_string()
        } else {
            self.rope.to_string()
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor_for_char(self.cursor)
    }

    pub fn cursor_char(&self) -> usize {
        self.cursor
    }

    pub fn cursor_for_char(&self, char_idx: usize) -> Cursor {
        let total_chars = self.rope.len_chars();
        if total_chars == 0 {
            return Cursor { line: 0, col: 0 };
        }
        if char_idx >= total_chars {
            let line = self.rope.len_lines().saturating_sub(1);
            let line_start = self.rope.line_to_char(line);
            let col = total_chars.saturating_sub(line_start);
            return Cursor { line, col };
        }
        let line = self.rope.char_to_line(char_idx);
        let line_start = self.rope.line_to_char(line);
        let col = char_idx.saturating_sub(line_start);
        Cursor { line, col }
    }

    pub fn find_next(&self, query: &str, start: usize) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        let text = self.rope.to_string();
        if text.is_empty() {
            return None;
        }
        let total_chars = text.chars().count();
        let start = start.min(total_chars);
        if let Some(idx) = find_in_text(&text, query, start) {
            return Some(idx);
        }
        if start > 0 {
            return find_in_text(&text, query, 0);
        }
        None
    }

    pub fn find_prev(&self, query: &str, start: usize) -> Option<usize> {
        if query.is_empty() {
            return None;
        }
        let text = self.rope.to_string();
        if text.is_empty() {
            return None;
        }
        let matches = find_all_in_text(&text, query);
        if matches.is_empty() {
            return None;
        }
        let total_chars = text.chars().count();
        let start = start.min(total_chars);
        if let Some((_, idx)) = matches
            .iter()
            .enumerate()
            .rev()
            .find(|(_, idx)| **idx < start)
        {
            return Some(*idx);
        }
        matches.last().copied()
    }

    pub fn ime_cursor_char(&self) -> usize {
        if let Some(preedit) = &self.preedit {
            if let Some((_, end)) = preedit.cursor {
                let in_preedit = preedit.text[..end.min(preedit.text.len())]
                    .chars()
                    .count();
                return self.cursor + in_preedit;
            }
        }
        self.cursor
    }

    #[allow(dead_code)]
    pub fn has_selection(&self) -> bool {
        self.selection_range().is_some()
    }

    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(if anchor < self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        })
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        Some(self.rope.slice(start..end).to_string())
    }

    pub fn delete_selection(&mut self) -> bool {
        let (start, end) = match self.selection_range() {
            Some(range) => range,
            None => return false,
        };
        self.preedit = None;
        let cursor_before = self.cursor;
        let deleted = self.remove_range(start, end);
        self.cursor = start;
        let edit = Edit {
            kind: EditKind::Delete { idx: start, text: deleted },
            cursor_before,
            cursor_after: self.cursor,
        };
        self.selection_anchor = None;
        self.push_undo(edit);
        self.dirty = true;
        true
    }

    pub fn select_all(&mut self) -> bool {
        let before_cursor = self.cursor;
        let before_selection = self.selection_range();
        let total_chars = self.rope.len_chars();
        if total_chars == 0 {
            self.selection_anchor = None;
            self.cursor = 0;
        } else {
            self.selection_anchor = Some(0);
            self.cursor = total_chars;
        }
        self.cursor != before_cursor || self.selection_range() != before_selection
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn encoding(&self) -> TextEncoding {
        self.encoding
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_preedit(&mut self, text: String, cursor: Option<(usize, usize)>) {
        if text.is_empty() {
            self.preedit = None;
        } else {
            self.preedit = Some(Preedit { text, cursor });
        }
    }

    pub fn clear_preedit(&mut self) {
        self.preedit = None;
    }

    pub fn commit_preedit(&mut self, text: &str) {
        self.preedit = None;
        self.insert_str(text);
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.preedit = None;
        let cursor_before = self.cursor;
        let edit = if let Some((start, end)) = self.selection_range() {
            let deleted = self.remove_range(start, end);
            self.cursor = start;
            self.rope.insert(start, text);
            let inserted_len = text.chars().count();
            self.cursor = start + inserted_len;
            Edit {
                kind: EditKind::Replace {
                    idx: start,
                    deleted,
                    inserted: text.to_string(),
                },
                cursor_before,
                cursor_after: self.cursor,
            }
        } else {
            self.rope.insert(self.cursor, text);
            let inserted_len = text.chars().count();
            self.cursor += inserted_len;
            Edit {
                kind: EditKind::Insert {
                    idx: cursor_before,
                    text: text.to_string(),
                },
                cursor_before,
                cursor_after: self.cursor,
            }
        };
        self.selection_anchor = None;
        self.push_undo(edit);
        self.dirty = true;
    }

    pub fn backspace(&mut self) {
        self.preedit = None;
        let cursor_before = self.cursor;
        let edit = if let Some((start, end)) = self.selection_range() {
            let deleted = self.remove_range(start, end);
            self.cursor = start;
            Edit {
                kind: EditKind::Delete { idx: start, text: deleted },
                cursor_before,
                cursor_after: self.cursor,
            }
        } else if self.cursor > 0 {
            let remove_start = self.cursor - 1;
            let deleted = self.remove_range(remove_start, self.cursor);
            self.cursor = remove_start;
            Edit {
                kind: EditKind::Delete {
                    idx: remove_start,
                    text: deleted,
                },
                cursor_before,
                cursor_after: self.cursor,
            }
        } else {
            return;
        };
        self.selection_anchor = None;
        self.push_undo(edit);
        self.dirty = true;
    }

    pub fn move_left(&mut self, extend: bool) {
        if self.cursor == 0 {
            return;
        }
        let next = self.cursor - 1;
        self.set_cursor(next, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        if self.cursor >= self.rope.len_chars() {
            return;
        }
        let next = self.cursor + 1;
        self.set_cursor(next, extend);
    }

    pub fn move_up(&mut self, extend: bool) {
        let cursor = self.cursor_for_char(self.cursor);
        if cursor.line == 0 {
            return;
        }
        let target_line = cursor.line - 1;
        let target_line_len = line_len_chars(&self.rope, target_line);
        let target_col = cursor.col.min(target_line_len);
        let next = self.rope.line_to_char(target_line) + target_col;
        self.set_cursor(next, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        let cursor = self.cursor_for_char(self.cursor);
        if cursor.line + 1 >= self.rope.len_lines() {
            return;
        }
        let target_line = cursor.line + 1;
        let target_line_len = line_len_chars(&self.rope, target_line);
        let target_col = cursor.col.min(target_line_len);
        let next = self.rope.line_to_char(target_line) + target_col;
        self.set_cursor(next, extend);
    }

    pub fn undo(&mut self) -> bool {
        let edit = match self.undo.pop() {
            Some(edit) => edit,
            None => return false,
        };
        self.apply_edit(&edit, false);
        self.redo.push(edit);
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let edit = match self.redo.pop() {
            Some(edit) => edit,
            None => return false,
        };
        self.apply_edit(&edit, true);
        self.undo.push(edit);
        self.trim_undo_history();
        self.dirty = true;
        true
    }

    pub fn load_from_bytes(&mut self, bytes: &[u8]) -> Result<TextEncoding, CoreError> {
        let (encoding, bom_len) = Encoding::for_bom(bytes).unwrap_or((UTF_8, 0));
        let encoding = TextEncoding::from_encoding(encoding).unwrap_or(TextEncoding::Utf8);
        let payload = &bytes[bom_len..];
        let (decoded, _, _) = encoding.encoding().decode(payload);
        self.rope = Rope::from_str(decoded.as_ref());
        self.cursor = 0;
        self.selection_anchor = None;
        self.preedit = None;
        self.undo.clear();
        self.redo.clear();
        self.encoding = encoding;
        self.dirty = false;
        Ok(encoding)
    }

    pub fn encode_text(text: &str, encoding: TextEncoding) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(encoding.bom());
        let (encoded, _, _) = encoding.encoding().encode(text);
        output.extend_from_slice(encoded.as_ref());
        output
    }

    pub fn mark_saved(&mut self, path: PathBuf, encoding: TextEncoding) {
        self.path = Some(path);
        self.encoding = encoding;
        self.dirty = false;
    }

    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
    }

    pub fn set_encoding(&mut self, encoding: TextEncoding) {
        self.encoding = encoding;
    }

    fn push_undo(&mut self, edit: Edit) {
        self.undo.push(edit);
        self.trim_undo_history();
        self.redo.clear();
    }

    fn trim_undo_history(&mut self) {
        if self.undo.len() <= Self::UNDO_LIMIT {
            return;
        }
        let overflow = self.undo.len() - Self::UNDO_LIMIT;
        self.undo.drain(0..overflow);
    }

    fn set_cursor(&mut self, next: usize, extend: bool) {
        if extend {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
        } else {
            self.selection_anchor = None;
        }
        self.cursor = next.min(self.rope.len_chars());
        if let Some(anchor) = self.selection_anchor {
            if anchor == self.cursor {
                self.selection_anchor = None;
            }
        }
    }

    fn remove_range(&mut self, start: usize, end: usize) -> String {
        if start >= end {
            return String::new();
        }
        let deleted = self.rope.slice(start..end).to_string();
        self.rope.remove(start..end);
        deleted
    }

    fn apply_edit(&mut self, edit: &Edit, forward: bool) {
        self.preedit = None;
        self.selection_anchor = None;
        match (&edit.kind, forward) {
            (EditKind::Insert { idx, text }, true) => {
                self.rope.insert(*idx, text);
            }
            (EditKind::Insert { idx, text }, false) => {
                let len = text.chars().count();
                self.rope.remove(*idx..*idx + len);
            }
            (EditKind::Delete { idx, text }, true) => {
                let len = text.chars().count();
                self.rope.remove(*idx..*idx + len);
            }
            (EditKind::Delete { idx, text }, false) => {
                self.rope.insert(*idx, text);
            }
            (EditKind::Replace { idx, deleted, inserted }, true) => {
                let del_len = deleted.chars().count();
                self.rope.remove(*idx..*idx + del_len);
                self.rope.insert(*idx, inserted);
            }
            (EditKind::Replace { idx, deleted, inserted }, false) => {
                let ins_len = inserted.chars().count();
                self.rope.remove(*idx..*idx + ins_len);
                self.rope.insert(*idx, deleted);
            }
        }
        self.cursor = if forward {
            edit.cursor_after
        } else {
            edit.cursor_before
        };
    }
}

impl CoreError {
    pub fn from_io(context: impl Into<String>, err: std::io::Error) -> Self {
        let kind = match err.kind() {
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::AlreadyExists
            | std::io::ErrorKind::PermissionDenied => SystemErrorKind::Permission,
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => {
                SystemErrorKind::Encoding
            }
            _ => SystemErrorKind::Io,
        };
        CoreError::System(SystemError {
            kind,
            context: format!("{}: {}", context.into(), err),
            retriable: matches!(
                err.kind(),
                std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
            ),
        })
    }

    pub fn describe(&self) -> String {
        match self {
            CoreError::System(err) => format!(
                "system error: {:?} (retriable={}) {}",
                err.kind, err.retriable, err.context
            ),
            CoreError::Domain(err) => {
                format!("domain error: {:?} {}", err.kind, err.context)
            }
        }
    }
}

fn line_len_chars(rope: &Rope, line: usize) -> usize {
    let line_text = rope.line(line);
    let len = line_text.len_chars();
    if line + 1 < rope.len_lines() && len > 0 {
        len - 1
    } else {
        len
    }
}

fn char_to_byte_idx(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| text.len())
}

fn find_in_text(text: &str, query: &str, start_char: usize) -> Option<usize> {
    let start_byte = char_to_byte_idx(text, start_char);
    if start_byte > text.len() {
        return None;
    }
    let found = text[start_byte..].find(query)?;
    let byte_idx = start_byte + found;
    Some(text[..byte_idx].chars().count())
}

pub(crate) fn find_all_in_text(text: &str, query: &str) -> Vec<usize> {
    let query_len = query.chars().count();
    if query_len == 0 {
        return Vec::new();
    }
    let total_chars = text.chars().count();
    let mut matches = Vec::new();
    let mut cursor = 0;
    while cursor <= total_chars {
        let Some(idx) = find_in_text(text, query, cursor) else {
            break;
        };
        matches.push(idx);
        let next = idx + query_len;
        if next <= cursor {
            cursor = cursor.saturating_add(1);
        } else {
            cursor = next;
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_for_char_clamps_empty_rope() {
        let core = Core::new();
        assert_eq!(core.cursor_for_char(0), Cursor { line: 0, col: 0 });
        assert_eq!(core.cursor_for_char(1), Cursor { line: 0, col: 0 });
    }

    #[test]
    fn cursor_for_char_at_end_returns_end_position() {
        let mut core = Core::new();
        core.insert_str("a");
        assert_eq!(core.cursor_for_char(1), Cursor { line: 0, col: 1 });
    }

    #[test]
    fn ime_cursor_char_with_preedit_on_empty_does_not_panic() {
        let mut core = Core::new();
        core.set_preedit("あ".to_string(), Some((3, 3)));
        let ime_idx = core.ime_cursor_char();
        assert_eq!(ime_idx, 1);
        assert_eq!(core.cursor_for_char(ime_idx), Cursor { line: 0, col: 0 });
    }

    #[test]
    fn line_count_reflects_inserted_newlines() {
        let mut core = Core::new();
        assert_eq!(core.line_count(), 1);
        core.insert_str("a\nb\nc");
        assert_eq!(core.line_count(), 3);
    }

    #[test]
    fn set_cursor_line_col_clamps_and_moves() {
        let mut core = Core::new();
        core.insert_str("a\nbc");
        assert!(core.set_cursor_line_col(1, 1, false));
        assert_eq!(core.cursor(), Cursor { line: 1, col: 1 });
        assert!(core.set_cursor_line_col(9, 9, false));
        assert_eq!(core.cursor(), Cursor { line: 1, col: 2 });
    }

    #[test]
    fn find_next_wraps_and_skips_start() {
        let mut core = Core::new();
        core.insert_str("abc def abc");
        assert_eq!(core.find_next("abc", 0), Some(0));
        assert_eq!(core.find_next("abc", 1), Some(8));
        assert_eq!(core.find_next("abc", 9), Some(0));
    }

    #[test]
    fn find_next_returns_none_on_empty_query() {
        let mut core = Core::new();
        core.insert_str("abc");
        assert_eq!(core.find_next("", 0), None);
    }

    #[test]
    fn find_all_collects_matches() {
        let mut core = Core::new();
        core.insert_str("abc def abc abc");
        let text = core.text();
        assert_eq!(find_all_in_text(&text, "abc"), vec![0, 8, 12]);
    }

    #[test]
    fn find_prev_wraps_to_last_match() {
        let mut core = Core::new();
        core.insert_str("abc def abc");
        assert_eq!(core.find_prev("abc", 0), Some(8));
        assert_eq!(core.find_prev("abc", 8), Some(0));
        assert_eq!(core.find_prev("abc", 9), Some(8));
    }

    #[test]
    fn select_all_on_empty_is_noop() {
        let mut core = Core::new();
        assert!(!core.select_all());
        assert_eq!(core.selection_range(), None);
        assert_eq!(core.cursor_char(), 0);
    }

    #[test]
    fn select_all_selects_full_range() {
        let mut core = Core::new();
        core.insert_str("a\nbc");
        assert!(core.select_all());
        let len = core.text().chars().count();
        assert_eq!(core.selection_range(), Some((0, len)));
        assert_eq!(core.cursor_char(), len);
        assert!(!core.select_all());
    }

    #[test]
    fn selected_text_returns_selection_contents() {
        let mut core = Core::new();
        core.insert_str("ab\ncd");
        core.set_cursor_line_col(0, 1, false);
        core.set_cursor_line_col(1, 1, true);
        assert_eq!(core.selected_text(), Some("b\nc".to_string()));
    }

    #[test]
    fn insert_str_replaces_selection() {
        let mut core = Core::new();
        core.insert_str("ab\ncd");
        core.set_cursor_line_col(0, 1, false);
        core.set_cursor_line_col(1, 1, true);
        core.insert_str("X");
        assert_eq!(core.text(), "aXd");
        assert_eq!(core.selection_range(), None);
    }

    #[test]
    fn delete_selection_removes_selected_text() {
        let mut core = Core::new();
        core.insert_str("ab\ncd");
        core.set_cursor_line_col(0, 1, false);
        core.set_cursor_line_col(1, 1, true);
        assert!(core.delete_selection());
        assert_eq!(core.text(), "ad");
        assert_eq!(core.cursor_char(), 1);
        assert_eq!(core.selection_range(), None);
    }

    #[test]
    fn undo_history_is_capped() {
        let mut core = Core::new();
        for _ in 0..101 {
            core.insert_str("a");
        }
        for _ in 0..Core::UNDO_LIMIT {
            assert!(core.undo());
        }
        assert!(!core.undo());
        assert_eq!(core.text(), "a");
    }

    #[test]
    fn line_len_chars_excludes_newline() {
        let mut core = Core::new();
        core.insert_str("ab\nc");
        assert_eq!(core.line_len_chars(0), 2);
        assert_eq!(core.line_len_chars(1), 1);
    }

}
