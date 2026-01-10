use ropey::Rope;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
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
}

impl Core {
    pub fn new() -> Self {
        Self {
            rope: Rope::from_str(""),
            cursor: 0,
            selection_anchor: None,
            preedit: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn display_text(&self) -> String {
        if let Some(preedit) = &self.preedit {
            let mut text = self.rope.to_string();
            let insert_at = char_to_byte_idx(&text, self.cursor);
            text.insert_str(insert_at, &preedit.text);
            text
        } else {
            self.rope.to_string()
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor_for_char(self.cursor)
    }

    pub fn cursor_for_char(&self, char_idx: usize) -> Cursor {
        let line = self.rope.char_to_line(char_idx);
        let line_start = self.rope.line_to_char(line);
        let col = char_idx.saturating_sub(line_start);
        Cursor { line, col }
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
        true
    }

    pub fn redo(&mut self) -> bool {
        let edit = match self.redo.pop() {
            Some(edit) => edit,
            None => return false,
        };
        self.apply_edit(&edit, true);
        self.undo.push(edit);
        true
    }

    fn push_undo(&mut self, edit: Edit) {
        self.undo.push(edit);
        self.redo.clear();
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
