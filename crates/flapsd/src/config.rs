//! TOML configuration for `flapsd`.
//!
//! [`Config`] is the deserialized form of the daemon's configuration file.
//! The HMAC pepper is NOT stored here: use [`read_pepper`] to read it from the
//! environment variable `FLAPS_HMAC_PEPPER` (fail-closed if absent or empty).

use std::net::SocketAddr;

use serde::Deserialize;

/// Default admin username when the field is omitted from the TOML.
fn default_admin_username() -> String {
    "admin".to_owned()
}

/// Daemon configuration, deserialized from a TOML file.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Database connection URL.
    ///
    /// Must start with `sqlite:` / `sqlite://` for SQLite, or `postgres://`
    /// for PostgreSQL.
    pub database_url: String,

    /// Address to bind the HTTP listener on (e.g. `"127.0.0.1:8080"`).
    ///
    /// Validated as a [`SocketAddr`] during [`Config::load`].
    pub bind_addr: String,

    /// Admin account username created on first boot (default: `"admin"`).
    #[serde(default = "default_admin_username")]
    pub admin_username: String,

    /// Optional per-minute rate limit for SDK endpoints.
    pub rate_limit_per_minute: Option<u32>,

    /// Optional session TTL in seconds.
    pub session_ttl_secs: Option<u64>,
}

/// Errors that can occur when loading or validating the configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read from disk.
    #[error("cannot read config file: {0}")]
    Io(#[from] std::io::Error),

    /// The file content is not valid TOML or does not match the expected schema.
    #[error("invalid TOML config: {0}")]
    Toml(#[from] toml::de::Error),

    /// The `bind_addr` field is not a valid socket address.
    #[error("invalid bind_addr {addr:?}: {source}")]
    InvalidBindAddr {
        /// The raw string that failed to parse.
        addr: String,
        /// The underlying parse error.
        source: std::net::AddrParseError,
    },

    /// The `database_url` field is empty or uses an unrecognised scheme.
    #[error("invalid database_url: {0}")]
    InvalidDatabaseUrl(String),

    /// The `FLAPS_HMAC_PEPPER` environment variable is absent or empty.
    #[error("FLAPS_HMAC_PEPPER is not set or is empty (fail-closed)")]
    PepperMissing,
}

impl Config {
    /// Reads and validates the TOML configuration from `path`.
    ///
    /// Validation rules:
    /// - `bind_addr` must parse as a [`SocketAddr`].
    /// - `database_url` must be non-empty and start with a recognised scheme
    ///   (`sqlite:` or `postgres://`).
    ///
    /// # Errors
    /// Returns [`ConfigError`] when the file cannot be read, the TOML is
    /// malformed, or a field fails validation.
    pub fn load(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Validates the parsed configuration.
    fn validate(&self) -> Result<(), ConfigError> {
        // Validate bind_addr.
        self.bind_addr
            .parse::<SocketAddr>()
            .map_err(|source| ConfigError::InvalidBindAddr {
                addr: self.bind_addr.clone(),
                source,
            })?;

        // Validate database_url.
        if self.database_url.is_empty() {
            return Err(ConfigError::InvalidDatabaseUrl(
                "database_url must not be empty".to_owned(),
            ));
        }
        let url = self.database_url.as_str();
        if !url.starts_with("sqlite:") && !url.starts_with("postgres://") {
            return Err(ConfigError::InvalidDatabaseUrl(format!(
                "unrecognised scheme in {url:?}; expected sqlite: or postgres://"
            )));
        }

        Ok(())
    }

    /// Returns the `bind_addr` parsed as a [`SocketAddr`].
    ///
    /// Panics if called before [`Config::load`] validates the field, but that
    /// cannot happen because `load` calls `validate` first.
    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        self.bind_addr
            .parse()
            .expect("bind_addr already validated in Config::load")
    }
}

/// Reads the HMAC pepper from the `FLAPS_HMAC_PEPPER` environment variable.
///
/// Fail-closed: returns [`ConfigError::PepperMissing`] when the variable is
/// absent or set to an empty string. The pepper is never read from the TOML
/// file or from any other source.
///
/// # Errors
/// Returns [`ConfigError::PepperMissing`] when the variable is not set or is
/// empty.
pub fn read_pepper() -> Result<Vec<u8>, ConfigError> {
    match std::env::var("FLAPS_HMAC_PEPPER") {
        Ok(v) if !v.is_empty() => Ok(v.into_bytes()),
        _ => Err(ConfigError::PepperMissing),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn write_toml(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile");
        f.write_all(content.as_bytes()).expect("write");
        f
    }

    // -- Config::load --

    #[test]
    fn load_valid_toml_succeeds() {
        let f = write_toml(
            r#"
database_url = "sqlite://flaps.db"
bind_addr    = "127.0.0.1:8080"
"#,
        );
        let cfg = Config::load(f.path().to_str().unwrap()).expect("load");
        assert_eq!(cfg.database_url, "sqlite://flaps.db");
        assert_eq!(cfg.bind_addr, "127.0.0.1:8080");
        assert_eq!(cfg.admin_username, "admin", "default admin_username");
    }

    #[test]
    fn load_explicit_admin_username() {
        let f = write_toml(
            r#"
database_url  = "postgres://user:pass@host/db"
bind_addr     = "0.0.0.0:9090"
admin_username = "superadmin"
"#,
        );
        let cfg = Config::load(f.path().to_str().unwrap()).expect("load");
        assert_eq!(cfg.admin_username, "superadmin");
    }

    #[test]
    fn load_invalid_toml_returns_err() {
        let f = write_toml("this is not TOML ===");
        let result = Config::load(f.path().to_str().unwrap());
        assert!(result.is_err(), "expected error on invalid TOML");
        assert!(
            matches!(result.unwrap_err(), ConfigError::Toml(_)),
            "expected Toml variant"
        );
    }

    #[test]
    fn load_invalid_bind_addr_returns_err() {
        let f = write_toml(
            r#"
database_url = "sqlite://flaps.db"
bind_addr    = "not-a-socket-addr"
"#,
        );
        let result = Config::load(f.path().to_str().unwrap());
        assert!(
            matches!(result, Err(ConfigError::InvalidBindAddr { .. })),
            "expected InvalidBindAddr"
        );
    }

    #[test]
    fn load_empty_database_url_returns_err() {
        let f = write_toml(
            r#"
database_url = ""
bind_addr    = "127.0.0.1:8080"
"#,
        );
        let result = Config::load(f.path().to_str().unwrap());
        assert!(
            matches!(result, Err(ConfigError::InvalidDatabaseUrl(_))),
            "expected InvalidDatabaseUrl"
        );
    }

    #[test]
    fn load_unknown_database_scheme_returns_err() {
        let f = write_toml(
            r#"
database_url = "mysql://user:pass@host/db"
bind_addr    = "127.0.0.1:8080"
"#,
        );
        let result = Config::load(f.path().to_str().unwrap());
        assert!(
            matches!(result, Err(ConfigError::InvalidDatabaseUrl(_))),
            "expected InvalidDatabaseUrl for unknown scheme"
        );
    }

    #[test]
    fn load_missing_file_returns_io_error() {
        let result = Config::load("/this/path/does/not/exist.toml");
        assert!(
            matches!(result, Err(ConfigError::Io(_))),
            "expected Io error"
        );
    }

    // -- read_pepper --

    #[test]
    fn read_pepper_present_returns_bytes() {
        temp_env::with_var("FLAPS_HMAC_PEPPER", Some("my-secret-pepper"), || {
            let result = read_pepper();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), b"my-secret-pepper");
        });
    }

    #[test]
    fn read_pepper_absent_returns_err() {
        temp_env::with_var("FLAPS_HMAC_PEPPER", None::<&str>, || {
            let result = read_pepper();
            assert!(
                matches!(result, Err(ConfigError::PepperMissing)),
                "expected PepperMissing when var absent"
            );
        });
    }

    #[test]
    fn read_pepper_empty_returns_err() {
        temp_env::with_var("FLAPS_HMAC_PEPPER", Some(""), || {
            let result = read_pepper();
            assert!(
                matches!(result, Err(ConfigError::PepperMissing)),
                "expected PepperMissing when var is empty"
            );
        });
    }
}
