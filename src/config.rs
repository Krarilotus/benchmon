//! User-owned host configuration lives beside the executable, never in the binary.
use crate::probe::Target;
use std::path::Path;

pub struct HostConfig {
    pub name: String,
    pub target: Target,
}

pub fn load() -> Result<Vec<HostConfig>, String> {
    let path = std::env::current_exe()
        .map_err(|error| format!("Cannot locate executable: {error}"))?
        .with_file_name("hosts.txt");
    load_path(&path)
}

fn load_path(path: &Path) -> Result<Vec<HostConfig>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => parse("This PC|local"),
        Err(error) => Err(format!("Cannot read hosts.txt: {error}")),
    }
}

fn parse(text: &str) -> Result<Vec<HostConfig>, String> {
    let mut hosts = Vec::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<_> = line.split('|').map(str::trim).collect();
        let fail = || {
            format!(
                "hosts.txt line {}: use name|local or name|windows/linux|SSH-destination",
                index + 1
            )
        };
        if parts[0].is_empty() {
            return Err(fail());
        }
        let target = match parts.as_slice() {
            [_, "local"] => Target::Local,
            [_, platform @ ("windows" | "linux"), destination]
                if !destination.is_empty() && !destination.starts_with('-') =>
            {
                if *platform == "windows" {
                    Target::Windows(destination.to_string())
                } else {
                    Target::Linux(destination.to_string())
                }
            }
            _ => return Err(fail()),
        };
        hosts.push(HostConfig { name: parts[0].into(), target });
    }
    if hosts.is_empty() {
        return Err("hosts.txt has no machines. Add This PC|local to monitor this computer.".into());
    }
    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_comments_bom_crlf_aliases_and_windows_usernames_with_spaces() {
        let hosts = parse("\u{feff}# my list\r\n Desktop | local\r\n Remote | windows | Example User@desktop.example\r\n Server|linux|my-server\r\n").unwrap();
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0].name, "Desktop");
        assert!(
            matches!(&hosts[1].target, Target::Windows(h) if h == "Example User@desktop.example")
        );
        assert!(matches!(&hosts[2].target, Target::Linux(h) if h == "my-server"));
    }

    #[test]
    fn rejects_bad_lines_and_ssh_options_without_silently_connecting_anywhere() {
        for text in [
            "",
            "# empty",
            "|local",
            "a|local|extra",
            "a|linux|",
            "a|windows|-oProxyCommand=anything",
            "a|unknown|host",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert!(parse("This PC|local\nbad").err().unwrap().contains("line 2"));
    }
}
