//! The one settings module: `worldprotector-settings.txt` in the plugin's data folder, read when the
//! plugin loads and on `/rg reload`. Settings never touch protection: a bad file only keeps the defaults.

use std::io::Read;
use std::path::Path;

pub const FILE: &str = "worldprotector-settings.txt";
const MAX_FILE: u64 = 4096;
const TEMPLATE: &str = "\
# worldprotector-lite settings; they apply at the next start or /rg reload. See the plugin README.
refusal-message=off
";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Tell a refused player so in chat. Off unless the server turns it on.
    pub refusal_message: bool,
}

/// The settings, or the number of the first line that cannot be read.
pub fn parse(text: &str) -> Result<Settings, usize> {
    let mut out = Settings::default();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.refusal_message = match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("refusal-message", "on")) => true,
            Some(("refusal-message", "off")) => false,
            _ => return Err(index + 1),
        };
    }
    Ok(out)
}

/// Writes the template when there is no file yet. A read-only folder still runs on the defaults.
pub fn load(dir: &Path) -> Result<Settings, String> {
    let path = dir.join(FILE);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = std::fs::write(&path, TEMPLATE);
            return Ok(Settings::default());
        }
        Err(_) => return Err(format!("{FILE} cannot be read")),
    };
    let mut text = String::new();
    file.take(MAX_FILE + 1)
        .read_to_string(&mut text)
        .map_err(|_| format!("{FILE} cannot be read"))?;
    if text.len() as u64 > MAX_FILE {
        return Err(format!("{FILE} is too long"));
    }
    parse(&text).map_err(|line| format!("{FILE}, line {line}: not a setting"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_refusal_message_is_off_unless_a_setting_turns_it_on() {
        assert!(!Settings::default().refusal_message);
        assert_eq!(parse(""), Ok(Settings::default()));
        assert_eq!(parse(TEMPLATE), Ok(Settings::default()));
        assert!(parse("refusal-message=on").unwrap().refusal_message);
        assert!(
            parse("# note\n\n refusal-message = on \n")
                .unwrap()
                .refusal_message
        );
        assert!(!parse("refusal-message=off").unwrap().refusal_message);
    }
    #[test]
    fn a_line_it_cannot_read_is_named_and_nothing_is_guessed() {
        assert_eq!(parse("refusal-message=yes"), Err(1));
        assert_eq!(parse("# note\nrefusal-message"), Err(2));
        assert_eq!(parse("refusal-message=on\nother=on"), Err(2));
    }
}
