use crate::error::Result;
use crate::input::{InputSeq, KeySeq};
use crate::prompt::{self, Prompt, PromptResult};
use crate::screen::Screen;
use crate::status_bar::StatusBar;
use crate::text_buffer::{CursorDir, TextBuffer};
use std::io::Write;
use std::path::Path;

enum EditStep {
    Continue(InputSeq),
    Quit,
}

impl EditStep {
    fn continues(&self) -> bool {
        match self {
            EditStep::Continue(_) => true,
            EditStep::Quit => false,
        }
    }
}

pub struct Editor<I: Iterator<Item = Result<InputSeq>>, W: Write> {
    input: I,
    quitting: bool,
    screen: Screen<W>,
    bufs: Vec<TextBuffer>,
    buf_idx: usize,
    status_bar: StatusBar,
}

impl<I, W> Editor<I, W>
where
    I: Iterator<Item = Result<InputSeq>>,
    W: Write,
{
    fn with_buf(
        buf: TextBuffer,
        input: I,
        output: W,
        window_size: Option<(u16, u16)>,
    ) -> Result<Editor<I, W>> {
        let screen = Screen::new(window_size, output)?;
        let status_bar = StatusBar::from_buffer(&buf, (1, 1));
        Ok(Editor {
            input,
            quitting: false,
            screen,
            bufs: vec![buf],
            buf_idx: 0,
            status_bar,
        })
    }
    
    pub fn new(input: I, output: W, window_size: Option<(u16, u16)>) -> Result<Editor<I, W>> {
        Self::with_buf(TextBuffer::empty(), input, output, window_size)
    }
    
    pub fn open<P: AsRef<Path>>(
        input: I,
        output: W,
        window_size: Option<(u16, u16)>,
        paths: &[P],
    ) -> Result<Editor<I, W>> {
        if paths.is_empty() {
            return Self::new(input, output, window_size);
        }
        let screen = Screen::new(window_size, output)?;
        let bufs: Vec<_> = paths.iter().map(TextBuffer::open).collect::<Result<_>>()?;
        let status_bar = StatusBar::from_buffer(&bufs[0], (1, bufs.len()));
        Ok(Editor {
            input,
            quitting: false,
            screen,
            bufs,
            buf_idx: 0,
            status_bar,
        })
    }

    pub fn buf(&self) -> &TextBuffer {
        &self.bufs[self.buf_idx]
    }

    pub fn buf_mut(&mut self) -> &mut TextBuffer {
        &mut self.bufs[self.buf_idx]
    }

    fn refresh_status_bar(&mut self) {
        self.status_bar.set_buf_pos((self.buf_idx + 1, self.bufs.len()));
        self.status_bar.update_from_buf(&self.bufs[self.buf_idx]);
    }

    fn render_screen(&mut self) -> Result<()> {
        self.refresh_status_bar();
        self.screen.render(&self.bufs[self.buf_idx], &self.status_bar)?;
        self.status_bar.redraw = false;
        Ok(())
    }

    fn handle_quit(&mut self, s: InputSeq) -> EditStep {
        let modified = self.bufs.iter().any(|b| b.modified());
        if !modified || self.quitting {
            EditStep::Quit
        } else {
            self.quitting = true;
            self.screen.set_error_message(
                "At least one file has unsaved changes! Press ^Q again to quit or ^S to save",
            );
            EditStep::Continue(s)
        }
    }

    fn process_keypress(&mut self, s: InputSeq) -> Result<EditStep> {
        use KeySeq::*;

        let prev_cursor = self.buf().cursor();
        
        match &s {
            InputSeq {
                key: Unidentified, ..
            } => return Ok(EditStep::Continue(s)),
            InputSeq { key, ctrl: true, ..
            } => match key {
                Key(b'd') => self.buf_mut().delete_right_char(),
                Key(b'h') => self.buf_mut().delete_char(),
                Key(b's') => self.save()?,
                Key(b'm') => {
                    self.buf_mut().insert_line()
                }
                Key(b'u') => {
                    if !self.buf_mut().undo() {
                        self.screen.set_info_message("No older change");
                    }
                }
                Key(b'r') => {
                    if !self.buf_mut().redo() {
                        self.screen.set_info_message("Buffer is already newest");
                    }
                }
                Key(b'q') => return Ok(self.handle_quit(s)),
                _ => {}
            }
            InputSeq { key, .. } => match key {
                Key(0x08) => self.buf_mut().delete_char(),
                Key(0x7f) => self.buf_mut().delete_char(),
                Key(b'\r') => self.buf_mut().insert_line(),
                Key(b) if !b.is_ascii_control() => self.buf_mut().insert_char(*b as char),
                Utf8Key(c) => self.buf_mut().insert_char(*c),
                UpKey => self.buf_mut().move_cursor_one(CursorDir::Up),
                LeftKey => self.buf_mut().move_cursor_one(CursorDir::Left),
                DownKey => self.buf_mut().move_cursor_one(CursorDir::Down),
                RightKey => self.buf_mut().move_cursor_one(CursorDir::Right),
                _ => {}
            }
        }

        if let Some(line) = self.buf_mut().finish_edit() {
            self.screen.set_dirty_start(line);
        }
        if self.buf().cursor() != prev_cursor {
            self.screen.cursor_moved = true;
        }
        
        self.quitting = false;
        Ok(EditStep::Continue(s))
    }

    fn save(&mut self) -> Result<()> {
        let mut create = false;
        if !self.buf().has_file() {
            let template = "Save as: {} (^G or ESC to cancel)";
            if let PromptResult::Input(input) = self.prompt::<prompt::NoAction>(template, true)? {
                self.buf_mut().set_file(input);
                create = true;
            } 
        }

        match self.buf_mut().save() {
            Ok(msg) => self.screen.set_info_message(msg),
            Err(msg) => {
                self.screen.set_error_message(msg);
                if create {
                    self.buf_mut().set_unnamed();
                }
            }
        }
        
        Ok(())
    }

    fn prompt<A: prompt::Action>(
        &mut self,
        prompt: &str,
        empty_is_cancel: bool,
    ) -> Result<PromptResult> {
        Prompt::new(
            &mut self.screen,
            &mut self.bufs[self.buf_idx],
            &mut self.status_bar,
            empty_is_cancel,
        )
        .run::<A, _, _>(prompt, &mut self.input)
    }
    
    fn step(&mut self) -> Result<EditStep> {
        let seq = if let Some(seq) = self.input.next() {
            seq?
        } else {
            return Ok(EditStep::Quit);
        };

        let step = self.process_keypress(seq)?;
        if step.continues() {
            self.render_screen()?;
        }
        
        Ok(step)
      
    }
    
    pub fn first_paint(&mut self) -> Result<Edit<'_, I, W>> {
        if self.buf().is_scratch() {
            self.screen.render_welcome(&self.status_bar)?;
            self.status_bar.redraw = false;
        } else {
            self.render_screen()?;
        }
        Ok(Edit { editor: self })
    }

    pub fn edit(&mut self) -> Result<()> {
        self.first_paint()?.try_for_each(|r| r.map(|_| ()))
    }
}

pub struct Edit<'a, I , W>
where
    I: Iterator<Item = Result<InputSeq>>,
    W: Write,
{
    editor: &'a mut Editor<I, W>,
}

impl<'a, I, W> Iterator for Edit<'a, I, W>
where
    I: Iterator<Item = Result<InputSeq>>,
    W: Write,
{
    type Item = Result<InputSeq>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.editor.step() {
            Ok(EditStep::Continue(seq)) => Some(Ok(seq)),
            Ok(EditStep::Quit) => None,
            Err(err) => Some(Err(err)),
        }
    }
}

