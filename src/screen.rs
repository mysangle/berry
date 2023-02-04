use crate::error::{Error, Result};
use crate::input::{InputSeq, KeySeq};
use crate::row::Row;
use crate::signal::SigwinchWatcher;
use crate::status_bar::StatusBar;
use crate::term_color::{Color};
use crate::text_buffer::TextBuffer;

use std::cmp;
use std::io::Write;
use std::time::SystemTime;
use unicode_width::UnicodeWidthChar;

use crossterm::{execute, cursor, terminal};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const HELP: &str = "\
    Ctrl-?              : Show this help";

#[derive(PartialEq)]
enum StatusMessageKind {
    Info,
    Error,
}

struct StatusMessage {
    text: String,
    timestamp: SystemTime,
    kind: StatusMessageKind,
}

impl StatusMessage {
    fn new<S: Into<String>>(message: S, kind: StatusMessageKind) -> StatusMessage {
        StatusMessage {
            text: message.into(),
            timestamp: SystemTime::now(),
            kind,
        }
    }
}

fn get_window_size() -> Result<(u16, u16)>
{
    terminal::size().map_err(|_| Error::UnknownWindowSize)
}

fn too_small_window(width: u16, height: u16) -> bool {
    width < 1 || height < 3    
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum DrawMessage {
    Open,
    Close,
    Update,
    DoNothing,
}

impl DrawMessage {
    fn fold(self, rhs: Self) -> Self {
        use DrawMessage::*;
        match (self, rhs) {
            (Open, Open) => unreachable!(),
            (Open, Close) => DoNothing,
            (Open, Update) => Open,
            (Close, Open) => Update,
            (Close, Close) => unreachable!(),
            (Close, Update) => unreachable!(),
            (Update, Open) => unreachable!(),
            (Update, Close) => Close,
            (Update, Update) => Update,
            (lhs, DoNothing) => lhs,
            (DoNothing, rhs) => rhs,
        }
    }
}

pub struct Screen<W: Write> {
    output: W,
    rx: usize,
    num_cols: usize,
    num_rows: usize,
    message: Option<StatusMessage>,
    draw_message: DrawMessage,
dirty_start: Option<usize>,
    sigwinch: SigwinchWatcher,
    pub cursor_moved: bool,
    pub rowoff: usize,
    pub coloff: usize,
}

impl<W: Write> Screen<W> {
    pub fn new(size: Option<(u16, u16)>, mut output: W) -> Result<Self> {
        let (w, h) = if let Some(s) = size {
            s
        } else {
            get_window_size()?
        };

        if too_small_window(w, h) {
            return Err(Error::TooSmallWindow(w, h));
        }
        
        execute!(output, terminal::EnterAlternateScreen)?;
        execute!(output, terminal::Clear(terminal::ClearType::All))?;

        Ok(Self {
            output,
            rx: 0,
            num_cols: w as usize,
            num_rows: h.saturating_sub(2) as usize,
            message: Some(StatusMessage::new(
                "Ctrl-? for help",
                StatusMessageKind::Info,
            )),
            draw_message: DrawMessage::Open,
            dirty_start: Some(0),
            sigwinch: SigwinchWatcher::new()?,
            cursor_moved: true,
            rowoff: 0,
            coloff: 0,
        })
    }

    fn write_flush(&mut self, bytes: &[u8]) -> Result<()> {
        self.output.write(bytes)?;
        self.output.flush()?;
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.output.write(bytes)?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.output.flush()?;
        Ok(())
    }

    fn trim_line<S: AsRef<str>>(&self, line: &S) -> String {
        let line = line.as_ref();
        if line.len() <= self.coloff {
            return "".to_string();
        }
        line.chars().skip(self.coloff).take(self.num_cols).collect()
    }

    fn draw_rows(
        &mut self,
        dirty_start: usize,
        rows: &[Row],
    ) -> Result<()> {
        let row_len = rows.len();

        for y in 0..self.rows() {
            let file_row = y + self.rowoff;

            if file_row < dirty_start {
                continue;
            }

            execute!(self.output, cursor::MoveTo(0, y as u16))?;

            let mut buf = Vec::with_capacity(0);
            if file_row >= row_len {
                self.write(b"~")?;
            } else {
                let row = &rows[file_row];

                let mut col = 0;
                for c in row.render_text().chars() {
                    col += c.width_cjk().unwrap_or(1);
                    if col <= self.coloff {
                        continue;
                    } else if col > self.num_cols + self.coloff {
                        break;
                    }
                    
                    write!(buf, "{}", c);
                }
            }

            self.write(&buf)?;
            self.write(b"\x1b[K")?;
        }
        
        Ok(())
    }

    fn next_coloff(&self, want_stop: usize, row: &Row) -> usize {
        let mut coloff = 0;
        for c in row.render_text().chars() {
            coloff += c.width_cjk().unwrap_or(1);
            if coloff >= want_stop {
                break;
            }
        }
        coloff
    }

    fn draw_status_bar<B: Write>(&self, mut buf: B, status_bar: &StatusBar) -> Result<()> {
        write!(buf, "\x1b[{}H", self.rows() + 1)?;

        let left = status_bar.left();
        let left = &left[..cmp::min(left.len(), self.num_cols)];
        buf.write(left.as_bytes())?;

        let rest_len = self.num_cols - left.len();
        if rest_len == 0 {
            return Ok(());
        }

        let right = status_bar.right();
        if right.len() > rest_len {
            for _ in 0..rest_len {
                buf.write(b" ")?;
            }
            return Ok(());
        }

        for _ in 0..rest_len - right.len() {
            buf.write(b" ")?;
        }
        buf.write(right.as_bytes())?;

        Ok(())
    }

    fn draw_message_bar<B: Write>(&self, mut buf: B, message: &StatusMessage) -> Result<()> {
        let text = &message.text[..cmp::min(message.text.len(), self.num_cols)];

        write!(buf, "\x1b[{}H", self.num_rows + 2)?;

        buf.write(text.as_bytes())?;
        buf.write(b"\x1b[K")?;
        Ok(())
    }

    fn do_scroll(&mut self, rows: &[Row], (cx, cy): (usize, usize)) {
        let prev_rowoff = self.rowoff;
        let prev_coloff = self.coloff;

        if cy < rows.len() {
            self.rx = rows[cy].rx_from_cx(cx);
        } else {
            self.rx = 0;
        }

        if cy < self.rowoff {
            self.rowoff = cy;
        }
        if cy >= self.rowoff + self.rows() {
            self.rowoff = cy - self.rows() + 1;
        }
        if self.rx < self.coloff {
            self.coloff = self.rx;
        }
        if self.rx >= self.coloff + self.num_cols {
            self.coloff = self.next_coloff(self.rx - self.num_cols + 1, &rows[cy]);
        }

        if prev_rowoff != self.rowoff || prev_coloff != self.coloff {
            self.set_dirty_start(self.rowoff);
        }
    }

    fn redraw(
        &mut self,
        text_buf: &TextBuffer,
        status_bar: &StatusBar,
    ) -> Result<()> {
        let cursor_row = text_buf.cy() - self.rowoff + 1;
        let cursor_col = self.rx - self.coloff + 1;
        let draw_message = self.draw_message;

        if self.dirty_start.is_none()
            && !status_bar.redraw
            && draw_message == DrawMessage::DoNothing
        {
            if self.cursor_moved {
                execute!(self.output, cursor::MoveTo((cursor_col - 1) as u16, (cursor_row - 1) as u16))?;
                self.output.flush()?;
            }
            return Ok(())
        }

        execute!(self.output, cursor::Hide)?;

        if let Some(s) = self.dirty_start {
            self.draw_rows(s, text_buf.rows())?;
        }

        if status_bar.redraw
            || draw_message == DrawMessage::Open
            || draw_message == DrawMessage::Close
        {
            let mut buf = Vec::with_capacity(1 * self.num_cols);
            self.draw_status_bar(&mut buf, status_bar)?;
            self.write(&buf)?;
        }

        if draw_message == DrawMessage::Update || draw_message == DrawMessage::Open {
            if let Some(message) = &self.message {
                let mut buf = Vec::with_capacity(1 * self.num_cols);
                self.draw_message_bar(&mut buf, message)?;
                self.write(&buf)?;
            }
        }

        execute!(self.output, cursor::MoveTo((cursor_col - 1) as u16, (cursor_row - 1) as u16))?;
        execute!(self.output, cursor::Show, cursor::SetCursorShape(cursor::CursorShape::Block))?;

        self.flush()?;

        Ok(())
    }

    fn after_render(&mut self) {
        self.dirty_start = None;
        self.cursor_moved = false;
        self.draw_message = DrawMessage::DoNothing;
    }

    pub fn render_welcome(&mut self, status_bar: &StatusBar) -> Result<()> {
        self.write_flush(b"\x1b[?25l")?;

        let mut buf = Vec::with_capacity((self.rows() + 2 + self.num_cols) * 3);

        for y in 0..self.rows() {
            write!(buf, "\x1b[{}H", y + 1)?;

            if y == self.rows() / 3 {
                let msg_buf = format!("Berry -- version {}", VERSION);
                let welcome = self.trim_line(&msg_buf);
                let padding = (self.num_cols - welcome.len()) / 2;
                if padding > 0 {
                    buf.write(b"~")?;
                    for _ in 0..padding - 1 {
                        buf.write(b" ")?;
                    }
                }
                buf.write(welcome.as_bytes())?;
            } else {
                if y == 0 {
                    buf.write(b" ")?;
                } else {
                    buf.write(b"~")?;
                }
            }

            buf.write(b"\x1b[K")?;
        }

        self.draw_status_bar(&mut buf, status_bar)?;
        if let Some(message) = &self.message {
            self.draw_message_bar(&mut buf, message)?;
        }
        
        write!(buf, "\x1b[H")?;
        buf.write(b"\x1b[?25h")?;
        self.write_flush(&buf);

        self.after_render();
        Ok(())
    }

    pub fn render(
        &mut self,
        buf: &TextBuffer,
        status_bar: &StatusBar,
    ) -> Result<()> {
        self.do_scroll(buf.rows(), buf.cursor());
        self.redraw(buf, status_bar)?;
        self.after_render();
        Ok(())
    }

    pub fn set_dirty_start(&mut self, start: usize) {
        if let Some(s) = self.dirty_start {
            if s < start {
                return;
            }
        }
        self.dirty_start = Some(start);
    }

    pub fn maybe_resize<I>(&mut self, input: I) -> Result<bool>
    where
        I: Iterator<Item = Result<InputSeq>>,
    {
        if !self.sigwinch.notified() {
            return Ok(false);
        }

        let (w, h) = get_window_size()?;
        if too_small_window(w, h) {
            return Err(Error::TooSmallWindow(w, h));
        }

        self.num_rows = h.saturating_sub(2) as usize;
        self.num_cols = w as usize;
        self.dirty_start = Some(0);

        Ok(true)
    }

    pub fn set_info_message<S: Into<String>>(&mut self, message: S) {
        self.set_message(Some(StatusMessage::new(message, StatusMessageKind::Info)));
    }

    pub fn set_error_message<S: Into<String>>(&mut self, message: S) {
        self.set_message(Some(StatusMessage::new(message, StatusMessageKind::Error)));
    }

    pub fn unset_message(&mut self) {
        self.set_message(None);
    }

    fn set_message(&mut self, m: Option<StatusMessage>) {
        let op = match (&self.message, &m) {
            (Some(p), Some(n)) if p.text == n.text => DrawMessage::DoNothing,
            (Some(_), Some(_)) => DrawMessage::Update,
            (None, Some(_)) => DrawMessage::Open,
            (Some(_), None) => DrawMessage::Close,
            (None, None) => DrawMessage::DoNothing,
        };

        self.draw_message = self.draw_message.fold(op);
        self.message = m;
    }

    pub fn rows(&self) -> usize {
        if self.message.is_some() {
            self.num_rows
        } else {
            self.num_rows + 1
        }
    }

    pub fn cols(&self) -> usize {
        self.num_cols
    }

    pub fn force_set_cursor(&mut self, row: usize, col: usize) -> Result<()> {
        write!(self.output, "\x1b[{};{}H", row, col)?;
        self.output.flush()?;
        Ok(())
    }
}

impl<W: Write> Drop for Screen<W> {
    fn drop(&mut self) {
        let _ = self.write_flush(b"\x1B[0 q");
        if let Err(err) = execute!(self.output, terminal::LeaveAlternateScreen) {
            eprintln!("Failed to leave alternate screen: {}", err);
        }
    }
}

