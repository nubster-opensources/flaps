//! TOML configuration for `flapsd`.
//!
//! [`Config`] is the deserialized form of the daemon's configuration file.
//! The HMAC pepper is NOT stored here: use [`read_pepper`] to read it from the
//! environment variable `FLAPS_HMAC_PEPPER` (fail-closed if absent or empty).

use std::net::SocketAddr;
use std::time::Duration;

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

    /// Per-minute SDK rate limit, applied to both the SQLite and PostgreSQL
    /// storage backends (default:
    /// [`DEFAULT_RATE_LIMIT_PER_MINUTE`](flaps_server::state::DEFAULT_RATE_LIMIT_PER_MINUTE)
    /// requests/minute when omitted).
    ///
    /// Must be greater than zero: use [`Config::effective_rate_limit_per_minute`]
    /// to read the value with the default applied. A zero value is rejected by
    /// [`Config::load`] as [`ConfigError::InvalidRateLimit`]; it has no
    /// "disable rate limiting" meaning, since a limiter that never lets a
    /// request through would be indistinguishable from a misconfiguration.
    pub rate_limit_per_minute: Option<u32>,

    /// Admin session TTL, in seconds (default:
    /// [`DEFAULT_SESSION_TTL_SECS`](flaps_server::state::DEFAULT_SESSION_TTL_SECS)
    /// seconds, i.e. 24 hours, when omitted).
    ///
    /// Controls how long a session minted by `POST /login` stays valid. Use
    /// [`Config::effective_session_ttl`] to read the value with the default
    /// applied. A zero value is rejected by [`Config::load`] as
    /// [`ConfigError::InvalidSessionTtl`]: a session that expires immediately
    /// is never useful.
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

    /// `rate_limit_per_minute` is set to zero.
    #[error(
        "invalid rate_limit_per_minute: must be greater than zero (omit the field to use the \
         default of {} requests/minute)",
        flaps_server::state::DEFAULT_RATE_LIMIT_PER_MINUTE
    )]
    InvalidRateLimit,

    /// `session_ttl_secs` is set to zero.
    #[error(
        "invalid session_ttl_secs: must be greater than zero (omit the field to use the \
         default of {} seconds)",
        flaps_server::state::DEFAULT_SESSION_TTL_SECS
    )]
    InvalidSessionTtl,
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

        // Validate rate_limit_per_minute: zero has no documented meaning, so it
        // is rejected rather than silently accepted as "no traffic allowed".
        if self.rate_limit_per_minute == Some(0) {
            return Err(ConfigError::InvalidRateLimit);
        }

        // Validate session_ttl_secs: a session that expires the instant it is
        // minted is never useful, so zero is rejected outright.
        if self.session_ttl_secs == Some(0) {
            return Err(ConfigError::InvalidSessionTtl);
        }

        Ok(())
    }

    /// Returns the effective SDK rate limit, in requests per minute.
    ///
    /// Falls back to
    /// [`DEFAULT_RATE_LIMIT_PER_MINUTE`](flaps_server::state::DEFAULT_RATE_LIMIT_PER_MINUTE)
    /// when [`Self::rate_limit_per_minute`] is omitted.
    #[must_use]
    pub fn effective_rate_limit_per_minute(&self) -> u32 {
        self.rate_limit_per_minute
            .unwrap_or(flaps_server::state::DEFAULT_RATE_LIMIT_PER_MINUTE)
    }

    /// Returns the effective admin session TTL.
    ///
    /// Falls back to
    /// [`DEFAULT_SESSION_TTL_SECS`](flaps_server::state::DEFAULT_SESSION_TTL_SECS)
    /// when [`Self::session_ttl_secs`] is omitted.
    #[must_use]
    pub fn effective_session_ttl(&self) -> Duration {
        Duration::from_secs(
            self.session_ttl_secs
                .unwrap_or(flaps_server::state::DEFAULT_SESSION_TTL_SECS),
        )
    }

    /// Returns the `bind_addr` parsed as a [`SocketAddr`].
    ///
    /// # Errors
    /// Returns [`ConfigError::InvalidBindAddr`] when the stored string cannot be
    /// parsed. In practice this cannot happen when the config was loaded through
    /// [`Config::load`], which validates the field upfront, but the fallible
    /// signature is necessary because `Config` is `Deserialize` and could be
    /// constructed without going through `load`.
    pub fn socket_addr(&self) -> Result<SocketAddr, ConfigError> {
        self.bind_addr
            .parse()
            .map_err(|source| ConfigError::InvalidBindAddr {
                addr: self.bind_addr.clone(),
                source,
            })
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

    // -- rate_limit_per_minute / session_ttl_secs --

    #[test]
    fn load_omitted_rate_limit_and_ttl_use_documented_defaults() {
        let f = write_toml(
            r#"
database_url = "sqlite://flaps.db"
bind_addr    = "127.0.0.1:8080"
"#,
        );
        let cfg = Config::load(f.path().to_str().unwrap()).expect("load");
        assert_eq!(cfg.rate_limit_per_minute, None);
        assert_eq!(cfg.session_ttl_secs, None);
        assert_eq!(
            cfg.effective_rate_limit_per_minute(),
            flaps_server::state::DEFAULT_RATE_LIMIT_PER_MINUTE,
            "omitted rate_limit_per_minute must fall back to the documented default"
        );
        assert_eq!(
            cfg.effective_session_ttl(),
            std::time::Duration::from_secs(flaps_server::state::DEFAULT_SESSION_TTL_SECS),
            "omitted session_ttl_secs must fall back to the documented default"
        );
    }

    #[test]
    fn load_explicit_rate_limit_and_ttl_are_applied() {
        let f = write_toml(
            r#"
database_url          = "sqlite://flaps.db"
bind_addr              = "127.0.0.1:8080"
rate_limit_per_minute  = 5
session_ttl_secs       = 120
"#,
        );
        let cfg = Config::load(f.path().to_str().unwrap()).expect("load");
        assert_eq!(cfg.effective_rate_limit_per_minute(), 5);
        assert_eq!(
            cfg.effective_session_ttl(),
            std::time::Duration::from_secs(120)
        );
    }

    #[test]
    fn load_zero_rate_limit_returns_err() {
        let f = write_toml(
            r#"
database_url          = "sqlite://flaps.db"
bind_addr              = "127.0.0.1:8080"
rate_limit_per_minute  = 0
"#,
        );
        let result = Config::load(f.path().to_str().unwrap());
        assert!(
            matches!(result, Err(ConfigError::InvalidRateLimit)),
            "expected InvalidRateLimit for a zero rate_limit_per_minute, got {result:?}"
        );
    }

    #[test]
    fn load_zero_session_ttl_returns_err() {
        let f = write_toml(
            r#"
database_url     = "sqlite://flaps.db"
bind_addr         = "127.0.0.1:8080"
session_ttl_secs  = 0
"#,
        );
        let result = Config::load(f.path().to_str().unwrap());
        assert!(
            matches!(result, Err(ConfigError::InvalidSessionTtl)),
            "expected InvalidSessionTtl for a zero session_ttl_secs, got {result:?}"
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
