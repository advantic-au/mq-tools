//! Resolution of the `--queue` / `--topic` arguments into an openable MQ target.

use mqi::types::{QueueManagerName, QueueName};

use crate::error::MqCatError;

/// The object a producer writes to or a consumer reads from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A named queue, optionally on a specific queue manager.
    Queue {
        /// Queue name.
        name: QueueName,
        /// Queue manager name, when the command line supplied one.
        queue_manager: Option<QueueManagerName>,
    },
    /// A topic string, which MQ resolves to a subscription when consumed.
    Topic {
        /// Topic string.
        name: String,
    },
}

impl Target {
    /// Build a target from raw command-line strings.
    ///
    /// # Errors
    /// Returns an error when a name cannot be represented in MQ's fixed-width
    /// fields, or when neither or both of the arguments are present.
    pub fn new(queue: Option<&str>, topic: Option<&str>, queue_manager: Option<&str>) -> Result<Self, MqCatError> {
        match (queue, topic) {
            (Some(name), None) => Ok(Self::Queue {
                name: parse_name::<QueueName>(name)?,
                queue_manager: queue_manager.map(parse_name::<QueueManagerName>).transpose()?,
            }),
            (None, Some(name)) => Ok(Self::Topic { name: name.to_owned() }),
            (None, None) => Err(MqCatError::MissingTarget),
            (Some(_), Some(_)) => Err(MqCatError::ConflictingTargets),
        }
    }

    /// Return the name as it should be displayed in logs and format strings.
    #[must_use]
    pub fn display_name(&self) -> String {
        match self {
            Self::Queue { name, .. } => name.to_string(),
            Self::Topic { name } => name.clone(),
        }
    }

    /// Return whether this target is a topic.
    #[must_use]
    pub const fn is_topic(&self) -> bool {
        matches!(self, Self::Topic { .. })
    }
}

/// Parse a fixed-width MQ name, reporting the offending input on failure.
///
/// Both `QueueName` and `QueueManagerName` wrap an `ObjectName` (`MqStr<48>`),
/// so the reported maximum length is the same for each.
fn parse_name<T>(value: &str) -> Result<T, MqCatError>
where
    T: std::str::FromStr<Err = mqi::string::MqStrError>,
{
    value.parse::<T>().map_err(|source| MqCatError::InvalidTargetName {
        name: value.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_target_parses() {
        let target = Target::new(Some("DEV.QUEUE.1"), None, Some("QM1")).unwrap();
        assert_eq!(target.display_name(), "DEV.QUEUE.1");
        assert!(!target.is_topic());
    }

    #[test]
    fn topic_target_parses() {
        let target = Target::new(None, Some("sport/tennis"), None).unwrap();
        assert_eq!(target.display_name(), "sport/tennis");
        assert!(target.is_topic());
    }

    #[test]
    fn missing_and_conflicting_targets_error() {
        assert!(matches!(Target::new(None, None, None), Err(MqCatError::MissingTarget)));
        assert!(matches!(
            Target::new(Some("Q1"), Some("t"), None),
            Err(MqCatError::ConflictingTargets)
        ));
    }

    #[test]
    fn overlong_queue_name_errors() {
        let long = "Q".repeat(49);
        assert!(matches!(
            Target::new(Some(&long), None, None),
            Err(MqCatError::InvalidTargetName { .. })
        ));
    }
}
