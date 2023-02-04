use getopts::Options;
use berry::{self as berry, Editor, StdinRawMode, HELP, VERSION};
use std::env;
use std::io;
use std::process::exit;

fn print_help(program: &str, opts: Options) {
    let description = format!(
        "{prog}: A tiny UTF-8 terminal text editor

Berry is a tiny UTF-8 text editor on terminals for Unix-like systems.
Specify file paths to edit as a command argument or run without argument to
start to write a new text.
Help can show up with key mapping Ctrl-?.

Usage:
    {prog} [options] [FILES...]

Mappings:
    {maps}",
        prog = program,
        maps = HELP,
    );
    println!("{}", opts.usage(&description)); 
}

fn edit(files: Vec<String>) -> berry::Result<()> {
    let input = StdinRawMode::new()?.input_keys();
    Editor::open(input, io::stdout(), None, &files)?.edit()
}

fn main() {
    let mut argv = env::args();
    let program = argv.next().unwrap();

    let mut opts = Options::new();
    opts.optflag("v", "version", "Print version");
    opts.optflag("h", "help", "Print this help");

    let matches = match opts.parse(argv) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error: {}, Please see --help for more details", e);
            exit(1);
        }
    };

    if matches.opt_present("v") {
        println!("{}", VERSION);
        return;
    }

    if matches.opt_present("h") {
        print_help(&program, opts);
        return;
    }

    if let Err(err) = edit(matches.free) {
        eprintln!("Error: {}", err);
        exit(1);
    }
}

