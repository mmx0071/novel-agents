use std::fmt;

#[derive(Debug, Clone)]
pub struct SchemaError {
    pub kind: &'static str,
    pub path: String,
    pub message: String,
}

impl SchemaError {
    pub fn new(kind: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.kind, self.path, self.message)
    }
}

impl std::error::Error for SchemaError {}
