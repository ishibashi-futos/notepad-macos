#[derive(Debug, Clone, Copy, Default)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
}

pub struct Core {
    lines: Vec<String>,
    cursor: Cursor,
}

impl Core {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor: Cursor::default(),
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn insert_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.insert_char(ch);
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        if ch == '\n' {
            let current = self.lines[self.cursor.line].clone();
            let (left, right) = split_at_char(&current, self.cursor.col);
            self.lines[self.cursor.line] = left;
            self.lines.insert(self.cursor.line + 1, right);
            self.cursor.line += 1;
            self.cursor.col = 0;
            return;
        }

        let line = &mut self.lines[self.cursor.line];
        let idx = byte_index(line, self.cursor.col);
        line.insert(idx, ch);
        self.cursor.col += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor.col > 0 {
            let line = &mut self.lines[self.cursor.line];
            let remove_at = self.cursor.col - 1;
            let idx = byte_index(line, remove_at);
            line.remove(idx);
            self.cursor.col -= 1;
            return;
        }

        if self.cursor.line == 0 {
            return;
        }

        let current = self.lines.remove(self.cursor.line);
        self.cursor.line -= 1;
        let line = &mut self.lines[self.cursor.line];
        let prev_len = line.chars().count();
        line.push_str(&current);
        self.cursor.col = prev_len;
    }

    pub fn move_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
            return;
        }
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.lines[self.cursor.line].chars().count();
        }
    }

    pub fn move_right(&mut self) {
        let line_len = self.lines[self.cursor.line].chars().count();
        if self.cursor.col < line_len {
            self.cursor.col += 1;
            return;
        }
        if self.cursor.line + 1 < self.lines.len() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor.line == 0 {
            return;
        }
        self.cursor.line -= 1;
        let line_len = self.lines[self.cursor.line].chars().count();
        self.cursor.col = self.cursor.col.min(line_len);
    }

    pub fn move_down(&mut self) {
        if self.cursor.line + 1 >= self.lines.len() {
            return;
        }
        self.cursor.line += 1;
        let line_len = self.lines[self.cursor.line].chars().count();
        self.cursor.col = self.cursor.col.min(line_len);
    }
}

fn byte_index(text: &str, col: usize) -> usize {
    text.char_indices()
        .nth(col)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| text.len())
}

fn split_at_char(text: &str, col: usize) -> (String, String) {
    let idx = byte_index(text, col);
    let (left, right) = text.split_at(idx);
    (left.to_string(), right.to_string())
}
