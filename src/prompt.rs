use crate::error::Result;
use crate::input::{InputSeq, KeySeq};
use crate::screen::Screen;
use crate::status_bar::StatusBar;
use crate::text_buffer::TextBuffer;

use std::io::Write;

#[derive(PartialEq)]
pub enum PromptResult {
    Canceled,
    Input(String),
}

pub trait Action: Sized {
    fn new<W: Write>(prompt: &mut Prompt<'_, W>) -> Self;

    fn on_seq<W: Write>(
        &mut self,
        _prompt: &mut Prompt<'_, W>,
        _input: &str,
        _seq: InputSeq,
    ) -> Result<bool> {
        Ok(false)
    }

    fn on_end<W: Write>(
        self,
        _prompt: &mut Prompt<'_, W>,
        result: PromptResult,
    ) -> Result<PromptResult> {
        Ok(result)
    }
}
pub struct NoAction;
impl Action for NoAction {
    fn new<W: Write>(_prompt: &mut Prompt<'_, W>) -> Self {
        Self
    }
}

struct PromptTemplate<'a> {
    prefix: &'a str,
    suffix: &'a str,
    prefix_chars: usize,
}

impl<'a> PromptTemplate<'a> {
    fn new(prefix: &'a str, suffix: &'a str) -> Self {
        let prefix_chars = prefix.chars().count();
        Self {
            prefix,
            suffix,
            prefix_chars,
        }
    }

    fn build(&self, input: &str) -> String {
        let cap = self.prefix.len() + self.suffix.len() + input.len();
        let mut buf = String::with_capacity(cap);
        buf.push_str(self.prefix);
        buf.push_str(input);
        buf.push_str(self.suffix);
        buf
    }

    fn cursor_col(&self, input: &str) -> usize {
        self.prefix_chars + input.chars().count() + 1
    }
}

pub struct Prompt<'a, W: Write> {
    screen: &'a mut Screen<W>,
    buf: &'a mut TextBuffer,
    sb: &'a mut StatusBar,
    empty_is_cancel: bool,
}

impl<'a, W: Write> Prompt<'a, W> {
    pub fn new<'s: 'a, 'tb: 'a, 'h: 'a, 'sb: 'a>(
        screen: &'s mut Screen<W>,
        buf: &'tb mut TextBuffer,
        sb: &'sb mut StatusBar,
        empty_is_cancel: bool,
    ) -> Self {
        Self { screen, buf, sb, empty_is_cancel }
    }

    fn render_screen(&mut self, input: &str, template: &PromptTemplate<'_>) -> Result<()> {
        self.screen.set_info_message(template.build(input));
        self.sb.update_from_buf(self.buf);
        self.screen.render(self.buf, self.sb)?;

        let row = self.screen.rows() + 2;
        let col = template.cursor_col(input);
        self.screen.force_set_cursor(row, col)?;

        self.sb.redraw = false;
        Ok(())
    }

    pub fn run<A, S, I>(&mut self, prompt: S, mut input: I) -> Result<PromptResult>
    where
        A: Action,
        S: AsRef<str>,
        I: Iterator<Item = Result<InputSeq>>,
    {
        let mut action = A::new(self);
        let mut buf = String::new();
        let mut canceled = false;

        let template = {
            let mut it = prompt.as_ref().splitn(2, "{}");
            let prefix = it.next().unwrap();
            let suffix = it.next().unwrap();
            PromptTemplate::new(prefix, suffix)
        };

        self.render_screen("", &template);

        while let Some(seq) = input.next() {
            use KeySeq::*;

            if self.screen.maybe_resize(&mut input)? {
                self.screen.set_dirty_start(self.screen.rowoff);
                self.sb.redraw = true;
                self.render_screen(&buf, &template)?;
                continue;
            }

            let seq = seq?;
            let prev_len = buf.len();

            match (&seq.key, &seq.ctrl) {
                (Unidentified, ..) => continue,
                (Key(b'h'), true) | (Key(0x7f), ..) | (DeleteKey, ..) => {
                    if !buf.is_empty() {
                        buf.pop();
                    }
                }
                (Key(b'g'), true) | (Key(b'q'), true) | (Key(0x1b), ..) => {
                    canceled = true;
                    break;
                }
                (Key(b'\r'), ..) | (Key(b'm'), true) => break,
                (Key(b'w'), true) => {
                    while let Some(current) = buf.pop() {
                        if let Some(next) = buf.chars().last() {
                            let next_is_not_char = next.is_ascii_punctuation() || next.is_ascii_whitespace();
                            let current_is_char = !current.is_ascii_punctuation() && !current.is_ascii_whitespace();
                            if current_is_char && next_is_not_char {
                                break;
                            }
                        }
                    }
                }
                (Key(b), false) => buf.push(*b as char),
                (Utf8Key(c), false) => buf.push(*c),
                _ => {}
            }

            let should_render = action.on_seq(self, buf.as_str(), seq)?;

            if should_render || prev_len != buf.len() {
                self.render_screen(&buf, &template)?;
            }
        }

        let result = if canceled || self.empty_is_cancel && buf.is_empty() {
            self.screen.set_info_message("Canceled");
            PromptResult::Canceled
        } else {
            self.screen.unset_message();
            self.sb.redraw = true;
            PromptResult::Input(buf)
        };
        
        action.on_end(self, result)
    }
}

