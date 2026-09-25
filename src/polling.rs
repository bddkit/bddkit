use crate::options::PollingOptions;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum AttemptError {
    NotYet(String),
    Fatal(String),
}

pub type AttemptResult = Result<(), AttemptError>;

impl AttemptError {
    pub fn into_message(self) -> String {
        match self {
            Self::NotYet(message) | Self::Fatal(message) => message,
        }
    }
}

impl From<String> for AttemptError {
    fn from(error: String) -> Self {
        Self::Fatal(error)
    }
}

pub struct Polling<'a> {
    assertion: &'a str,
    started: tokio::time::Instant,
    deadline: tokio::time::Instant,
    timeout: Duration,
    interval: Duration,
    attempts: u64,
}

impl<'a> Polling<'a> {
    pub fn new(assertion: &'a str, options: &PollingOptions) -> Self {
        let started = tokio::time::Instant::now();
        Self {
            assertion,
            started,
            deadline: started + options.timeout,
            timeout: options.timeout,
            interval: options.interval,
            attempts: 0,
        }
    }

    pub async fn after_not_yet(&mut self, last: &str) -> Result<(), String> {
        self.attempts += 1;
        let now = tokio::time::Instant::now();
        if now >= self.deadline {
            let elapsed = now.saturating_duration_since(self.started);
            return Err(format!(
                "{} did not pass within {:?} (interval {:?}, {} attempts, elapsed {:?}): {}",
                self.assertion, self.timeout, self.interval, self.attempts, elapsed, last
            ));
        }
        tokio::time::sleep((self.deadline - now).min(self.interval)).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::PollingOptions;
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn wakes_for_a_final_attempt_at_the_deadline() {
        let options = PollingOptions {
            timeout: Duration::from_millis(250),
            interval: Duration::from_millis(100),
        };
        let started = Instant::now();
        let mut polling = Polling::new("variable assertion", &options);

        assert!(polling.after_not_yet("first").await.is_ok());
        assert_eq!(Instant::now() - started, Duration::from_millis(100));
        assert!(polling.after_not_yet("second").await.is_ok());
        assert_eq!(Instant::now() - started, Duration::from_millis(200));
        assert!(polling.after_not_yet("third").await.is_ok());
        assert_eq!(Instant::now() - started, Duration::from_millis(250));

        let error = polling.after_not_yet("still missing").await.unwrap_err();
        assert!(error.contains("4 attempts"), "{error}");
        assert!(error.contains("still missing"), "{error}");
    }
}
