mod app;
mod core;
mod ui;

use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    let (path, extra_args) = parse_cli_args(std::env::args_os());
    let extra_warning = if extra_args.is_empty() {
        None
    } else {
        Some(format!(
            "extra arguments are ignored: {}",
            extra_args.join(", ")
        ))
    };
    app::App::run(path, extra_warning);
}

fn parse_cli_args<I>(args: I) -> (Option<PathBuf>, Vec<String>)
where
    I: IntoIterator<Item = OsString>,
{
    let mut iter = args.into_iter();
    let _program = iter.next();
    let path = iter.next().map(PathBuf::from);
    let mut extra = Vec::new();
    for arg in iter {
        extra.push(arg.to_string_lossy().into_owned());
    }
    (path, extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_args_handles_no_path() {
        let (path, extra) = parse_cli_args(vec![OsString::from("notepad-macos")]);
        assert!(path.is_none());
        assert!(extra.is_empty());
    }

    #[test]
    fn parse_cli_args_handles_path_and_extras() {
        let (path, extra) = parse_cli_args(vec![
            OsString::from("notepad-macos"),
            OsString::from("foo.txt"),
            OsString::from("bar.txt"),
        ]);
        assert_eq!(path, Some(PathBuf::from("foo.txt")));
        assert_eq!(extra, vec!["bar.txt".to_string()]);
    }
}
