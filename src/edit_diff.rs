use crate::row::Row;

#[derive(Debug, Clone, Copy)]
pub enum UndoRedo {
    Undo,
    Redo,
}

#[derive(Debug)]
pub enum EditDiff {
    InsertChar(usize, usize, char),
    DeleteChar(usize, usize, char),
    Append(usize, String),
    Truncate(usize, String),
    Newline,
    InsertLine(usize, String),
    DeleteLine(usize, String),
}

impl EditDiff {
    pub fn apply(&self, rows: &mut Vec<Row>, which: UndoRedo) -> (usize, usize) {
        use UndoRedo::*;
        match *self {
            EditDiff::InsertChar(x, y, c) => match which {
                Redo => {
                    rows[y].insert_char(x, c);
                    (x + 1, y)
                }
                Undo => {
                    rows[y].remove_char(x);
                    (x, y)
                }
            }
            EditDiff::DeleteChar(x, y, c) => match which {
                Redo => {
                    rows[y].remove_char(x - 1);
                    (x - 1, y)
                }
                Undo => {
                    rows[y].insert_char(x - 1, c);
                    (x, y)
                }
            }
            EditDiff::Append(y, ref s) => match which {
                Redo => {
                    let len = rows[y].len();
                    rows[y].append(s);
                    (len, y)
                }
                Undo => {
                    let count = s.chars().count();
                    let len = rows[y].len();
                    rows[y].remove(len - count, len);
                    (rows[y].len(), y)
                }
            }
            EditDiff::Truncate(y, ref s) => match which {
                Redo => {
                    let count = s.chars().count();
                    let len = rows[y].len();
                    rows[y].truncate(len - count);
                    (len - count, y)
                }
                Undo => {
                    rows[y].append(s);
                    let x = rows[y].len() - s.chars().count();
                    (x, y)
                }
            }
            EditDiff::Newline => match which {
                Redo => {
                    rows.push(Row::empty());
                    (0, rows.len() - 1)
                }
                Undo => {
                    debug_assert_eq!(rows[rows.len() - 1].buffer(), "");
                    rows.pop();
                    (0, rows.len())
                }
            }
            EditDiff::InsertLine(y, ref s) => match which {
                Redo => {
                    rows.insert(y, Row::new(s).unwrap());
                    (0, y)
                }
                Undo => {
                    rows.remove(y);
                    (rows[y - 1].len(), y - 1)
                }
            }
            EditDiff::DeleteLine(y, ref s) => match which {
                Redo => {
                    if y == rows.len() - 1 {
                        rows.pop();
                    } else {
                        rows.remove(y);
                    }
                    (rows[y - 1].len(), y - 1)
                }
                Undo => {
                    if y == rows.len() {
                        rows.push(Row::new(s).unwrap());
                    } else {
                        rows.insert(y, Row::new(s).unwrap());
                    }
                    (0, y)
                }
            }
        }
    }
}

