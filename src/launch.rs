use std::process::{Command, Stdio};

#[derive(Debug)]
pub enum LaunchError {
    EmptyCommand,
    SpawnError(std::io::Error),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::EmptyCommand => write!(f, "empty command"),
            LaunchError::SpawnError(e) => write!(f, "failed to spawn: {}", e),
        }
    }
}

impl std::error::Error for LaunchError {}

fn parse_bedrock_prefix(input: &str) -> (Option<String>, &str) {
    let input = input.trim();
    if let Some(colon) = input.find(':') {
        let prefix = input[..colon].trim().to_lowercase();
        let rest = input[colon + 1..].trim();
        if rest.is_empty() {
            return (None, input);
        }
        let stratum = match prefix.as_str() {
            "arch" => "arch",
            "deb" => "debian",
            "fed" => "fedora",
            other => other,
        };
        (Some(stratum.to_string()), rest)
    } else {
        (None, input)
    }
}

pub fn launch_background(input: &str) -> Result<(), LaunchError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(LaunchError::EmptyCommand);
    }

    let (stratum, command) = parse_bedrock_prefix(trimmed);
    if command.is_empty() {
        return Err(LaunchError::EmptyCommand);
    }

    let mut cmd = match stratum {
        Some(s) => {
            let mut c = Command::new("strat");
            c.arg(s).arg(command);
            c
        }
        None => {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        }
    };

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(LaunchError::SpawnError)
}
