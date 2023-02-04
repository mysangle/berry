use crate::error::{Error, Result};

use std::fmt;
use std::io::{self, Read};
use std::ops::{Deref, DerefMut};
use std::str;

use crossterm::{terminal};

pub struct StdinRawMode {
    stdin: io::Stdin,
}

impl StdinRawMode {
    pub fn new() -> Result<StdinRawMode> {
        let stdin = io::stdin();
        terminal::enable_raw_mode()?;

        Ok(StdinRawMode { stdin })
    }    

    pub fn input_keys(self) -> InputSequences {
        InputSequences { stdin: self }
    }
}

impl Drop for StdinRawMode {
    fn drop(&mut self) {
        if let Err(err) = terminal::disable_raw_mode() {
            eprintln!("Failed to disable raw mode: {}", err);
        }
    }
}

impl Deref for StdinRawMode {
    type Target = io::Stdin;

    fn deref(&self) -> &Self::Target {
        &self.stdin
    }    
}

impl DerefMut for StdinRawMode {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stdin
    }
}

#[derive(PartialEq, Debug, Clone)]
pub enum KeySeq {
    Unidentified,
    Utf8Key(char),
    Key(u8),
    LeftKey,
    RightKey,
    UpKey,
    DownKey,
    DeleteKey,
    Cursor(usize, usize),
}

impl fmt::Display for KeySeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use KeySeq::*;
        match self {
            Unidentified => write!(f, "UNKNOWN"),
            Key(b) if b.is_ascii_control() => write!(f, "\\x{:x}", b),
            Key(b) => write!(f, "{}", *b as char),
            Utf8Key(c) => write!(f, "{}", c),
            LeftKey => write!(f, "LEFT"),
            RightKey => write!(f, "RIGHT"),
            UpKey => write!(f, "UP"),
            DownKey => write!(f, "DOWN"),
            DeleteKey => write!(f, "DELETE"),
            Cursor(r, c) => write!(f, "CURSOR({}, {})", r, c),
        }
    }
}

#[derive(PartialEq, Debug, Clone)]
pub struct InputSeq {
    pub key: KeySeq,
    pub ctrl: bool,
    pub alt: bool,
}

impl InputSeq {
    pub fn new(key: KeySeq) -> Self {
        Self {
            key,
            ctrl: false,
            alt: false,
        }
    }

    pub fn ctrl(key: KeySeq) -> Self {
        Self {
            key,
            ctrl: true,
            alt: false,
        }
    }
}

pub struct InputSequences {
    stdin: StdinRawMode,
}

impl InputSequences {
    fn read_byte(&mut self) -> Result<Option<u8>> {
        let mut one_byte: [u8; 1] = [0];
        Ok(if self.stdin.read(&mut one_byte)? == 0 {
            None
        } else {                
            Some(one_byte[0])
        })
    }

    fn decode_escape_sequence(&mut self) -> Result<InputSeq> {
        use KeySeq::*;

        match self.read_byte()? {
            Some(b'[') => { /* fall through */ }
            Some(b) if b.is_ascii_control() => {
                return Ok(InputSeq::new(Key(0x1b)));
            }
            Some(b) => {
                let mut seq = self.decode(b)?;
                seq.alt = true;
                return Ok(seq);
            }
            None => return Ok(InputSeq::new(Key(0x1b))),
        };

        let mut buf = vec![];
        let cmd = loop {
            if let Some(b) = self.read_byte()? {
                match b {
                    b'A' | b'B' | b'C' | b'D' | b'F' | b'H' | b'K' | b'J' | b'R' | b'c' | b'f'
                    | b'g' | b'h' | b'l' | b'm' | b'n' | b'q' | b't' | b'y' | b'~' => break b,
                    _ => buf.push(b),
                }
            } else {
                return Ok(InputSeq::new(Unidentified));
            }
        };

        let mut args = buf.split(|b| *b == b';');
        match cmd {
            b'A' | b'B' | b'C' | b'D' => {
                let key = match cmd {
                    b'A' => UpKey,
                    b'B' => DownKey,
                    b'C' => RightKey,
                    b'D' => LeftKey,
                    _ => unreachable!(),
                };
                let ctrl = args.next() == Some(b"1") && args.next() == Some(b"5");
                let alt = false;
                Ok(InputSeq { key, ctrl, alt })
            }
            b'~' => {
                match args.next() {
                    Some(b"3") => Ok(InputSeq::new(DeleteKey)),
                    _ => Ok(InputSeq::new(Unidentified)),
                }
            }
            _ => unreachable!(),
        }
    }
    
    fn decode_utf8(&mut self, b: u8) -> Result<InputSeq> {
        let mut buf = [0; 4];
        buf[0] = b;
        let mut len = 1;

        loop {
            if let Some(b) = self.read_byte()? {
                buf[len] = b;
                len += 1;
            } else {
                return Err(Error::NotUtf8Input(buf[..len].to_vec()));
            }

            if let Ok(s) = str::from_utf8(&buf) {
                return Ok(InputSeq::new(KeySeq::Utf8Key(s.chars().next().unwrap())));
            }

            if len == 4 {
                return Err(Error::NotUtf8Input(buf.to_vec()));
            }
        }
    }
    
    fn decode(&mut self, b: u8) -> Result<InputSeq> {
        use KeySeq::*;
        
        match b {
            0x00..=0x1f => match b {
                0x1b => self.decode_escape_sequence(),
                0x00 | 0x1f => {
                    Ok(InputSeq::ctrl(Key(b | 0b0010_0000)))
                },
                0x01c | 0x01d => {
                    Ok(InputSeq::ctrl(Key(b | 0b0100_0000)))
                },
                _ => {
                    Ok(InputSeq::ctrl(Key(b | 0b0110_0000)))
                },
            },
            0x20..=0x7f => Ok(InputSeq::new(Key(b))),
            0x80..=0x9f => Ok(InputSeq::new(Unidentified)),
            0xa0..=0xff => self.decode_utf8(b),
        }
    }
    
    fn read_seq(&mut self) -> Result<InputSeq> {
        if let Some(b) = self.read_byte()? {
            self.decode(b)
        } else {
            Ok(InputSeq::new(KeySeq::Unidentified))
        }
    }
}

impl Iterator for InputSequences {
    type Item = Result<InputSeq>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.read_seq())
    }
}

