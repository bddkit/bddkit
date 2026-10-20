use crate::failure::StepError;
use crate::options::PollingOptions;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum AttemptError {
    NotYet(StepError),
    Fatal(StepError),
}

pub type AttemptResult = Result<(), AttemptError>;

impl AttemptError {
    pub fn into_error(self) -> StepError {
        match self {
            Self::NotYet(error) | Self::Fatal(error) => error,
        }
    }

    /// A check that did not hold, yet: polling may try again.
    pub fn not_yet(message: impl Into<String>) -> Self {
        Self::NotYet(StepError::assertion(message))
    }

    /// A check that did not hold and cannot change before the next attempt.
    pub fn assertion(message: impl Into<String>) -> Self {
        Self::Fatal(StepError::assertion(message))
    }

    /// The step itself is wrong: no attempt can fix it.
    pub fn input(message: impl Into<String>) -> Self {
        Self::Fatal(StepError::input(message))
    }
}

impl From<StepError> for AttemptError {
    fn from(error: StepError) -> Self {
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

    /// Sleeps before the next attempt, or fails once the deadline has passed.
    /// The failure keeps the class of the last attempt's: an assertion that
    /// never held is `assertion`, a server that stayed down is `transport`.
    pub async fn after_not_yet(&mut self, last: &StepError) -> Result<(), StepError> {
        self.attempts += 1;
        let now = tokio::time::Instant::now();
        if now >= self.deadline {
            let elapsed = now.saturating_duration_since(self.started);
            return Err(StepError::new(
                last.kind,
                format!(
                    "{} did not pass within {:?} (interval {:?}, {} attempts, elapsed {:?}): {}",
                    self.assertion, self.timeout, self.interval, self.attempts, elapsed, last
                ),
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

        assert!(
            polling
                .after_not_yet(&StepError::assertion("first"))
                .await
                .is_ok()
        );
        assert_eq!(Instant::now() - started, Duration::from_millis(100));
        assert!(
            polling
                .after_not_yet(&StepError::assertion("second"))
                .await
                .is_ok()
        );
        assert_eq!(Instant::now() - started, Duration::from_millis(200));
        assert!(
            polling
                .after_not_yet(&StepError::assertion("third"))
                .await
                .is_ok()
        );
        assert_eq!(Instant::now() - started, Duration::from_millis(250));

        let error = polling
            .after_not_yet(&StepError::assertion("still missing"))
            .await
            .unwrap_err();
        assert!(error.message.contains("4 attempts"), "{error}");
        assert!(error.message.contains("still missing"), "{error}");
        assert_eq!(error.kind, crate::failure::FailureKind::Assertion);
    }
}
