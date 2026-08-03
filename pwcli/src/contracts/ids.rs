use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

string_id!(SessionId);
string_id!(TaskId);
string_id!(AttemptId);
string_id!(WorkItemId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_keep_the_historical_string_wire_shape() {
        let id = SessionId::new("session-1");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"session-1\"");
        assert_eq!(
            serde_json::from_str::<SessionId>("\"session-1\"").unwrap(),
            id
        );
    }
}
