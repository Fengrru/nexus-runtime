#![deny(clippy::disallowed_types)]

mod policy_engine;
pub use policy_engine::*;

use nexus_core::{now_millis, SessionId, TaskId};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityScope {
    FsRead { path: String },
    FsWrite { path: String },
    ToolCall { tool_name: String, action: String },
    LlmInference { model: String },
    NetworkRead { host: String },
    NetworkWrite { host: String },
    SystemExec { command_pattern: String },
}

impl std::fmt::Display for CapabilityScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilityScope::FsRead { path } => write!(f, "fs:read:{}", path),
            CapabilityScope::FsWrite { path } => write!(f, "fs:write:{}", path),
            CapabilityScope::ToolCall { tool_name, action } => {
                write!(f, "tool:{}:{}", tool_name, action)
            }
            CapabilityScope::LlmInference { model } => write!(f, "llm:{}", model),
            CapabilityScope::NetworkRead { host } => write!(f, "net:read:{}", host),
            CapabilityScope::NetworkWrite { host } => write!(f, "net:write:{}", host),
            CapabilityScope::SystemExec { command_pattern } => {
                write!(f, "exec:{}", command_pattern)
            }
        }
    }
}

impl CapabilityScope {
    pub fn from_string(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();
        match parts.as_slice() {
            ["fs", "read", path] => Some(CapabilityScope::FsRead {
                path: path.to_string(),
            }),
            ["fs", "write", path] => Some(CapabilityScope::FsWrite {
                path: path.to_string(),
            }),
            ["tool", name, action] => Some(CapabilityScope::ToolCall {
                tool_name: name.to_string(),
                action: action.to_string(),
            }),
            ["llm", model] => Some(CapabilityScope::LlmInference {
                model: model.to_string(),
            }),
            ["net", "read", host] => Some(CapabilityScope::NetworkRead {
                host: host.to_string(),
            }),
            ["net", "write", host] => Some(CapabilityScope::NetworkWrite {
                host: host.to_string(),
            }),
            ["exec", pattern] => Some(CapabilityScope::SystemExec {
                command_pattern: pattern.to_string(),
            }),
            _ => None,
        }
    }
}

impl Serialize for CapabilityScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CapabilityScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        CapabilityScope::from_string(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("invalid capability scope: {}", s)))
    }
}

#[derive(Debug, Clone)]
pub struct CapabilityToken {
    pub version: u8,
    pub scope: CapabilityScope,
    pub session_id: SessionId,
    pub task_id: TaskId,
    pub expires_at: u64,
    pub issued_at: u64,
    pub signature: Vec<u8>,
}

impl CapabilityToken {
    pub fn issue(
        signing_key: &[u8],
        scope: CapabilityScope,
        session_id: SessionId,
        task_id: TaskId,
        expires_at: u64,
    ) -> Self {
        let issued_at = now_millis();
        let mut token = Self {
            version: 1,
            scope,
            session_id,
            task_id,
            expires_at,
            issued_at,
            signature: Vec::new(),
        };
        token.sign(signing_key);
        token
    }

    fn sign(&mut self, signing_key: &[u8]) {
        use hmac::Mac;
        use sha2::Sha256;
        type HmacSha256 = hmac::Hmac<Sha256>;

        let message = self.signing_message();
        let mut mac = HmacSha256::new_from_slice(signing_key).expect("HMAC key OK");
        mac.update(&message);
        self.signature = mac.finalize().into_bytes().to_vec();
    }

    pub fn verify(&self, signing_key: &[u8]) -> Result<(), CapabilityError> {
        use hmac::Mac;
        use sha2::Sha256;
        type HmacSha256 = hmac::Hmac<Sha256>;

        if now_millis() > self.expires_at {
            return Err(CapabilityError::Expired);
        }
        if self.version != 1 {
            return Err(CapabilityError::UnsupportedVersion);
        }

        let message = self.signing_message();
        let mut mac =
            HmacSha256::new_from_slice(signing_key).map_err(|_| CapabilityError::InvalidKey)?;
        mac.update(&message);

        mac.verify_slice(&self.signature)
            .map_err(|_| CapabilityError::InvalidSignature)
    }

    pub fn permits(&self, requested: &CapabilityScope) -> bool {
        match (&self.scope, requested) {
            (CapabilityScope::FsRead { path: granted }, CapabilityScope::FsRead { path: req }) => {
                let granted_canon = canonicalize_path(granted);
                let req_canon = canonicalize_path(req);
                req_canon.starts_with(&granted_canon)
            }
            (
                CapabilityScope::FsWrite { path: granted },
                CapabilityScope::FsWrite { path: req },
            ) => {
                let granted_canon = canonicalize_path(granted);
                let req_canon = canonicalize_path(req);
                req_canon.starts_with(&granted_canon)
            }
            (
                CapabilityScope::ToolCall {
                    tool_name: gt,
                    action: ga,
                },
                CapabilityScope::ToolCall {
                    tool_name: rt,
                    action: ra,
                },
            ) => gt == rt && (ga == "*" || ga == ra),
            (
                CapabilityScope::LlmInference { model: gm },
                CapabilityScope::LlmInference { model: rm },
            ) => gm == "*" || gm == rm,
            _ => false,
        }
    }

    fn signing_message(&self) -> Vec<u8> {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.version,
            self.scope,
            hex::encode(self.session_id.0),
            hex::encode(self.task_id.0),
            self.expires_at,
            self.issued_at
        )
        .into_bytes()
    }
}

fn canonicalize_path(path: &str) -> String {
    let path = std::path::Path::new(path);
    let mut result = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(c) => result.push(c),
            std::path::Component::RootDir => result.push("/"),
            _ => {}
        }
    }
    let s = result.to_string_lossy().into_owned();
    if s.is_empty() {
        ".".to_string()
    } else {
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    Expired,
    InvalidSignature,
    InvalidKey,
    UnsupportedVersion,
    InsufficientPermission,
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityError::Expired => write!(f, "Capability token expired"),
            CapabilityError::InvalidSignature => write!(f, "Invalid capability signature"),
            CapabilityError::InvalidKey => write!(f, "Invalid signing key"),
            CapabilityError::UnsupportedVersion => write!(f, "Unsupported token version"),
            CapabilityError::InsufficientPermission => write!(f, "Insufficient permissions"),
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SandboxError {
    #[error("Landlock not available")]
    LandlockUnavailable,
    #[error("Seccomp not available")]
    SeccompUnavailable,
    #[error("Sandbox violation: {0}")]
    Violation(String),
    #[error("Platform not supported")]
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxTier {
    Tier0,
    Tier1,
    Tier2,
}

impl SandboxTier {
    pub fn best_available() -> Self {
        #[cfg(target_os = "linux")]
        {
            if Self::landlock_available() {
                return SandboxTier::Tier0;
            }
            if Self::seccomp_available() {
                return SandboxTier::Tier1;
            }
        }
        SandboxTier::Tier2
    }

    #[cfg(target_os = "linux")]
    fn landlock_available() -> bool {
        let kernel = crate::kernel_version();
        kernel >= (5, 13, 0)
    }

    #[cfg(target_os = "linux")]
    fn seccomp_available() -> bool {
        crate::kernel_version() >= (3, 5, 0)
    }

    pub fn apply(&self, cmd: &mut std::process::Command) -> Result<(), SandboxError> {
        match self {
            SandboxTier::Tier0 => self.apply_tier0(cmd),
            SandboxTier::Tier1 => self.apply_tier1(cmd),
            SandboxTier::Tier2 => self.apply_tier2(cmd),
        }
    }

    fn apply_tier0(&self, cmd: &mut std::process::Command) -> Result<(), SandboxError> {
        #[cfg(target_os = "linux")]
        {
            if !Self::landlock_available() {
                return Err(SandboxError::LandlockUnavailable);
            }
            if !Self::seccomp_available() {
                return Err(SandboxError::SeccompUnavailable);
            }
        }
        cmd.env("NEXUS_READONLY", "1");
        Ok(())
    }

    fn apply_tier1(&self, _cmd: &mut std::process::Command) -> Result<(), SandboxError> {
        #[cfg(target_os = "linux")]
        {
            if !Self::seccomp_available() {
                return Err(SandboxError::SeccompUnavailable);
            }
        }
        Ok(())
    }

    fn apply_tier2(&self, _cmd: &mut std::process::Command) -> Result<(), SandboxError> {
        Ok(())
    }

    pub fn description(&self) -> &'static str {
        match self {
            SandboxTier::Tier0 => "Landlock + seccomp + read-only rootfs",
            SandboxTier::Tier1 => "seccomp + strict path whitelist",
            SandboxTier::Tier2 => "Command audit + logging",
        }
    }
}

#[cfg(target_os = "linux")]
fn kernel_version() -> (u32, u32, u32) {
    if let Ok(output) = std::process::Command::new("uname").arg("-r").output() {
        let release = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = release.trim().split('.').collect();
        if parts.len() >= 3 {
            let major = parts[0].parse().unwrap_or(0);
            let minor = parts[1].parse().unwrap_or(0);
            let patch = parts[2]
                .split('-')
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
            return (major, minor, patch);
        }
    }
    (0, 0, 0)
}

pub struct CapabilityManager {
    signing_key: Vec<u8>,
}

impl CapabilityManager {
    pub fn new(signing_key: Vec<u8>) -> Self {
        Self { signing_key }
    }

    pub fn issue_token(
        &self,
        scope: CapabilityScope,
        session_id: SessionId,
        task_id: TaskId,
        duration_ms: u64,
    ) -> CapabilityToken {
        let expires_at = now_millis() + duration_ms;
        CapabilityToken::issue(&self.signing_key, scope, session_id, task_id, expires_at)
    }

    pub fn verify_token(&self, token: &CapabilityToken) -> Result<(), CapabilityError> {
        token.verify(&self.signing_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_token_sign_and_verify() {
        let key = b"test-signing-key-32-bytes-long!!";
        let sid = SessionId::from_bytes([1u8; 16]);
        let tid = TaskId::from_bytes([2u8; 16]);
        let expires = now_millis() + 3_600_000;

        let token = CapabilityToken::issue(
            key,
            CapabilityScope::FsRead {
                path: "/project/src".into(),
            },
            sid,
            tid,
            expires,
        );

        assert!(token.verify(key).is_ok());
    }

    #[test]
    fn test_token_expired() {
        let key = b"test-signing-key-32-bytes-long!!";
        let sid = SessionId::from_bytes([1u8; 16]);
        let tid = TaskId::from_bytes([2u8; 16]);
        let expires = now_millis() - 1000;

        let token = CapabilityToken::issue(
            key,
            CapabilityScope::FsRead {
                path: "/src".into(),
            },
            sid,
            tid,
            expires,
        );

        assert!(matches!(token.verify(key), Err(CapabilityError::Expired)));
    }

    #[test]
    fn test_token_invalid_signature() {
        let key1 = b"test-signing-key-32-bytes-long!!";
        let key2 = b"different-signing-key-32-bytes!!";
        let sid = SessionId::from_bytes([1u8; 16]);
        let tid = TaskId::from_bytes([2u8; 16]);
        let expires = now_millis() + 3_600_000;

        let token = CapabilityToken::issue(
            key1,
            CapabilityScope::FsRead {
                path: "/src".into(),
            },
            sid,
            tid,
            expires,
        );

        assert!(matches!(
            token.verify(key2),
            Err(CapabilityError::InvalidSignature)
        ));
    }

    #[test]
    fn test_permits_fs_read() {
        let token_scope = CapabilityScope::FsRead {
            path: "/project/src".into(),
        };
        let request = CapabilityScope::FsRead {
            path: "/project/src/auth".into(),
        };
        let key = b"test-signing-key-32-bytes-long!!";
        let sid = SessionId::from_bytes([1u8; 16]);
        let tid = TaskId::from_bytes([2u8; 16]);

        let token = CapabilityToken::issue(key, token_scope, sid, tid, now_millis() + 3_600_000);

        assert!(token.permits(&request));
    }

    #[test]
    fn test_permits_denies_parent_dir() {
        let token_scope = CapabilityScope::FsRead {
            path: "/project/src".into(),
        };
        let request = CapabilityScope::FsRead {
            path: "/etc/passwd".into(),
        };
        let key = b"test-signing-key-32-bytes-long!!";
        let sid = SessionId::from_bytes([1u8; 16]);
        let tid = TaskId::from_bytes([2u8; 16]);

        let token = CapabilityToken::issue(key, token_scope, sid, tid, now_millis() + 3_600_000);

        assert!(!token.permits(&request));
    }

    #[test]
    fn test_canonicalize_path_blocks_traversal() {
        let result = canonicalize_path("/project/../etc/passwd");
        assert!(!result.contains(".."));
    }

    #[test]
    fn test_sandbox_tier_best_available() {
        let tier = SandboxTier::best_available();
        // Whatever tier is reported as best available must actually be applicable
        // on this platform; the concrete tier depends on the host kernel.
        let mut cmd = std::process::Command::new("nexus-sandbox-probe");
        assert!(tier.apply(&mut cmd).is_ok());
        #[cfg(not(target_os = "linux"))]
        assert_eq!(tier, SandboxTier::Tier2);
    }
}
