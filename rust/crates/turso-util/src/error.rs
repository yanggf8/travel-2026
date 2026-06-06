pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    kind: Kind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Auth,
    NotFound,
    Conflict,
    Validation,
    UnsupportedTier,
    Schema,
    Turso,
    Internal,
    Environment,
}

impl Error {
    pub fn auth(m: impl Into<String>) -> Self {
        Self::new(Kind::Auth, m)
    }

    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(Kind::NotFound, m)
    }

    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(Kind::Conflict, m)
    }

    pub fn validation(m: impl Into<String>) -> Self {
        Self::new(Kind::Validation, m)
    }

    pub fn schema(m: impl Into<String>) -> Self {
        Self::new(Kind::Schema, m)
    }

    pub fn turso(m: impl Into<String>) -> Self {
        Self::new(Kind::Turso, m)
    }

    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::new(Kind::Internal, e.to_string())
    }

    pub fn environment(m: impl Into<String>) -> Self {
        Self::new(Kind::Environment, m)
    }

    pub fn unsupported_tier(tier: &str, registry: &str) -> Self {
        Self::new(
            Kind::UnsupportedTier,
            format!("tier '{tier}' is not supported for registry '{registry}'"),
        )
    }

    fn new(kind: Kind, m: impl Into<String>) -> Self {
        Self {
            kind,
            message: m.into(),
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self.kind {
            Kind::Auth => "auth",
            Kind::NotFound => "not-found",
            Kind::Conflict => "conflict",
            Kind::Validation => "validation",
            Kind::UnsupportedTier => "unsupported-tier",
            Kind::Schema => "schema",
            Kind::Turso => "turso",
            Kind::Internal => "internal",
            Kind::Environment => "environment",
        }
    }

    pub fn exit_code_u8(&self) -> u8 {
        match self.kind {
            Kind::Auth => 3,
            Kind::NotFound => 4,
            Kind::Conflict => 5,
            Kind::Validation | Kind::UnsupportedTier => 2,
            Kind::Schema => 6,
            Kind::Environment => 8,
            Kind::Turso | Kind::Internal => 1,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Consumer supplies its own program name (was hardcoded "persona-core").
    pub fn formatted(&self, program: &str) -> String {
        format!("{program}: {}: {}", self.kind_str(), self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_tier_is_its_own_class_with_exit_two() {
        let e = Error::unsupported_tier("secrets", "finance-registry");
        assert_eq!(e.kind_str(), "unsupported-tier");
        assert_eq!(e.exit_code_u8(), 2);
        assert!(e.message().contains("secrets"));
        assert!(e.message().contains("finance-registry"));
    }

    #[test]
    fn emit_uses_supplied_program_name() {
        let e = Error::validation("bad");
        assert_eq!(e.formatted("gwebcdb"), "gwebcdb: validation: bad");
    }
}
