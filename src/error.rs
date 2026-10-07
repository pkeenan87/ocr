//! Error type mapped to the process exit codes (PLAN §3).

use std::fmt;

#[derive(Debug)]
pub enum CliError {
    /// Exit 1: some pages flagged or failed.
    Partial(String),
    /// Exit 2: bad arguments or input.
    Usage(String),
    /// Exit 2: not set up, or engine not installed.
    Setup(String),
    /// Exit 3: model server unavailable.
    Server(String),
}

impl CliError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Partial(_) => 1,
            Self::Usage(_) | Self::Setup(_) => 2,
            Self::Server(_) => 3,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) => write!(f, "error: {m}"),
            Self::Partial(m) | Self::Setup(m) | Self::Server(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CliError {}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> Self {
        Self::Setup(format!("error: {e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(CliError::Partial(String::new()).exit_code(), 1);
        assert_eq!(CliError::Usage(String::new()).exit_code(), 2);
        assert_eq!(CliError::Setup(String::new()).exit_code(), 2);
        assert_eq!(CliError::Server(String::new()).exit_code(), 3);
    }
}
