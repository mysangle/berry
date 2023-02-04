use crate::edit_diff::{EditDiff, UndoRedo};
use crate::error::Result;
use crate::history::History;
use crate::row::Row;

use std::fs::File;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

pub struct FilePath {
    pub path: PathBuf,
    pub display: String,
}

impl FilePath {
    fn from<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref();
        FilePath {
            path: PathBuf::from(path),
            display: path.to_string_lossy().to_string(),
        }
    }

    fn from_string<S: Into<String>>(s: S) -> Self {
        let display = s.into();
        FilePath {
            path: PathBuf::from(&display),
            display,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum CursorDir {
    Left,
    Right,
    Up,
    Down,
}

pub struct TextBuffer {
    cx: usize,
    cy: usize,
    file: Option<FilePath>,
    row: Vec<Row>,
    undo_count: i32,
    modified: bool,
    history: History,
    inserted_undo: bool,
    dirty_start: Option<usize>,
}

impl TextBuffer {
    pub fn empty() -> Self {
        Self {
            cx: 0,
            cy: 0,
            file: None,
            row: vec![Row::empty()],
            undo_count: 0,
            modified: false,
            history: History::default(),
            inserted_undo: false,
            dirty_start: Some(0),
        }
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let file = Some(FilePath::from(path));
        if !path.exists() {
            let mut buf = Self::empty();
            buf.file = file;
            buf.undo_count = 0;
            buf.modified = false;
            return Ok(buf);
        }

        let row = io::BufReader::new(File::open(path)?)
            .lines()
            .map(|r| Row::new(r?))
            .collect::<Result<_>>()?;
        
        Ok(Self {
            cx: 0,
            cy: 0,
            file,
            row,
            undo_count: 0,
            modified: false,
            history: History::default(),
            inserted_undo: false,
            dirty_start: Some(0),
        })
    }

    pub fn cy(&self) -> usize {
        self.cy
    }

    pub fn filename(&self) -> &str {
        self.file
            .as_ref()
            .map(|f| f.display.as_str())
            .unwrap_or("[No Name]")
    }

    pub fn has_file(&self) -> bool {
        self.file.is_some()
    }

    pub fn modified(&self) -> bool {
        self.undo_count != 0 || self.modified
    }

    pub fn set_file<S: Into<String>>(&mut self, file_path: S) {
        let file = FilePath::from_string(file_path);
        self.file = Some(file);
    }

    pub fn set_unnamed(&mut self) {
        self.file = None;
    }

    pub fn save(&mut self) -> std::result::Result<String, String> {
        self.insert_undo_point();

        let file = if let Some(file) = &self.file {
            file
        } else {
            return Ok("".to_string());
        };

        let f = match File::create(&file.path) {
            Ok(f) => f,
            Err(e) => return Err(format!("Could not save: {}", e)),
        };
        let mut f = io::BufWriter::new(f);
        let mut bytes = 0;
        for line in self.row.iter() {
            let b = line.buffer();
            writeln!(f, "{}", b).map_err(|e| format!("Could not write to file: {}", e))?;
            bytes += b.as_bytes().len() + 1;
        }
        f.flush().map_err(|e| format!("Could not flush to file: {}", e))?;

        self.undo_count = 0;
        self.modified = false;
        Ok(format!("{} bytes written to {}", bytes, &file.display))
    }

    fn set_dirty_start(&mut self, line: usize) {
        if let Some(l) = self.dirty_start {
            if l <= line {
                return;
            }
        }
        self.dirty_start = Some(line);
    }

    fn apply_diff(&mut self, diff: &EditDiff, which: UndoRedo) {
        let (x, y) = diff.apply(&mut self.row, which);
        self.set_cursor(x, y);
        self.set_dirty_start(y);
    }

    fn new_diff(&mut self, diff: EditDiff) {
        self.apply_diff(&diff, UndoRedo::Redo);
        self.modified = true;
        self.history.push(diff);
    }

    fn insert_undo_point(&mut self) {
        if !self.inserted_undo {
            if self.history.finish_ongoing_edit() {
                self.undo_count = self.undo_count.saturating_add(1);
            }
            self.modified = false;
            self.inserted_undo = true;
        }
    }

    pub fn finish_edit(&mut self) -> Option<usize> {
        self.inserted_undo = false;
        let dirty_start = self.dirty_start;
        self.dirty_start = None;
        dirty_start
    }

    pub fn insert_char(&mut self, ch: char) {
        if self.cy == self.row.len() {
            self.new_diff(EditDiff::Newline);
        }
        self.new_diff(EditDiff::InsertChar(self.cx, self.cy, ch));
    }

    pub fn delete_right_char(&mut self) {
        if self.cy == self.row.len()
            || self.cy == self.row.len() - 1 && self.cx == self.row[self.cy].len() {
            return;
        }
        self.move_cursor_one(CursorDir::Right);
        self.delete_char();
    }

    pub fn delete_char(&mut self) {
        if self.cy == self.row.len() || self.cx == 0 && self.cy == 0 {
            return;
        }
        self.insert_undo_point();
        if self.cx > 0 {
            let idx = self.cx - 1;
            let deleted = self.row[self.cy].char_at(idx);
            self.new_diff(EditDiff::DeleteChar(self.cx, self.cy, deleted));
        } else {
            self.squash_to_previous_line();
        }
    }

    pub fn insert_line(&mut self) {
        self.insert_undo_point();
        if self.cy >= self.row.len() {
            self.new_diff(EditDiff::Newline);
        } else if self.cx >= self.row[self.cy].len() {
            self.new_diff(EditDiff::InsertLine(self.cy + 1, "".to_string()));
        } else if self.cx <= self.row[self.cy].buffer().len() {
            let truncated = self.row[self.cy][self.cx..].to_owned();
            self.new_diff(EditDiff::Truncate(self.cy, truncated.clone()));
            self.new_diff(EditDiff::InsertLine(self.cy + 1, truncated));
        }
    }

    pub fn move_cursor_one(&mut self, dir: CursorDir) {
        match dir {
            CursorDir::Up => self.cy = self.cy.saturating_sub(1),
            CursorDir::Left => {
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.row[self.cy].len();
                }
            }
            CursorDir::Down => {
                if self.cy < self.row.len() {
                    self.cy += 1;
                }
            }
            CursorDir::Right => {
                if self.cy < self.row.len() {
                    let len = self.row[self.cy].len();
                    if self.cx < len {
                        self.cx += 1;
                    } else if self.cx >= len {
                        self.cy += 1;
                        self.cx = 0;
                    }
                }
            }
        };

        let len = self.row.get(self.cy).map(Row::len).unwrap_or(0);
        if self.cx > len {
            self.cx = len;
        }
    }

    fn squash_to_previous_line(&mut self) {
        self.cy -= 1;
        self.cx = self.row[self.cy].len();
        self.concat_next_line();
    }

    fn concat_next_line(&mut self) {
        let removed = self.row[self.cy + 1].buffer().to_owned();
        self.new_diff(EditDiff::DeleteLine(self.cy + 1, removed.clone()));
        self.new_diff(EditDiff::Append(self.cy, removed));
    }

    pub fn set_cursor(&mut self, x: usize, y: usize) {
        self.cx = x;
        self.cy = y;
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cx, self.cy)
    }

    pub fn rows(&self) -> &[Row] {
        &self.row
    }

    pub fn undo(&mut self) -> bool {
        let state = self.history.undo(&mut self.row);
        if let Some((_, _, _, edited)) = state {
            if !edited {
                self.undo_count = self.undo_count.saturating_sub(1);
            }
            self.modified = false;
        }
        self.after_undoredo(state)
    }

    pub fn redo(&mut self) -> bool {
        let state = self.history.redo(&mut self.row);
        if let Some((_, _, _, edited)) = state {
            if !edited {
                self.undo_count = self.undo_count.saturating_add(1);
            }
            self.modified = false;
        }
        self.after_undoredo(state)
    }

    fn after_undoredo(&mut self, state: Option<(usize, usize, usize, bool)>) -> bool {
        match state {
            Some((x, y, s, _)) => {
                self.set_cursor(x, y);
                self.set_dirty_start(s);
                true
            }
            None => false,
        }
    }

    pub fn is_scratch(&self) -> bool {
        self.file.is_none() && self.row.len() == 1 && self.row[0].len() == 0
    }
}

