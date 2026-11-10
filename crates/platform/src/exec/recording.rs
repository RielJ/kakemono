//! [`RecordingRunner`], the [`CommandRunner`] test double. Behind the `fake` feature (and this
//! crate's own tests), so it is not part of the production API.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{CommandRunner, Invocation, Output, RunError};
use crate::api::CommandStatus;

/// Test double: records every invocation and returns scripted outputs.
///
/// Clones share state. Outputs queued with [`push_output`](Self::push_output) are taken in call
/// order (when each `run` starts), then [`Output::success`]. With [`set_delay`](Self::set_delay), each `run` sleeps
/// (on the tokio clock, so paused-time tests work); a delay past the invocation's timeout
/// returns `TimedOut` after the timeout instead.
#[derive(Clone, Debug, Default)]
pub struct RecordingRunner {
    state: Arc<Mutex<RecordingState>>,
}

#[derive(Debug, Default)]
struct RecordingState {
    invocations: Vec<Invocation>,
    detached: Vec<Vec<String>>,
    outputs: VecDeque<Result<Output, String>>,
    delay: Duration,
    in_flight: usize,
    max_in_flight: usize,
}

/// First fake PID returned by `RecordingRunner::spawn_detached`.
pub const FAKE_PID_BASE: u32 = 40_000;

impl RecordingRunner {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, RecordingState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue the output for the next unscripted `run`.
    pub fn push_output(&self, output: Output) {
        self.state().outputs.push_back(Ok(output));
    }

    /// Make the next `run` fail to start, with this io error message.
    pub fn push_spawn_error(&self, message: impl Into<String>) {
        self.state().outputs.push_back(Err(message.into()));
    }

    /// Make each `run` take this long.
    pub fn set_delay(&self, delay: Duration) {
        self.state().delay = delay;
    }

    /// Every `run` call so far, in call order (recorded when the call starts).
    pub fn invocations(&self) -> Vec<Invocation> {
        self.state().invocations.clone()
    }

    /// Just the argv of every `run` call.
    pub fn argvs(&self) -> Vec<Vec<String>> {
        self.state()
            .invocations
            .iter()
            .map(|i| i.argv.clone())
            .collect()
    }

    /// Every `spawn_detached` argv. The n-th call returned PID `FAKE_PID_BASE + n`.
    pub fn detached(&self) -> Vec<Vec<String>> {
        self.state().detached.clone()
    }

    /// Runs currently in progress.
    pub fn in_flight(&self) -> usize {
        self.state().in_flight
    }

    /// The highest number of concurrent runs seen.
    pub fn max_in_flight(&self) -> usize {
        self.state().max_in_flight
    }
}

/// Decrements `in_flight` even if the run future is dropped.
struct InFlight(RecordingRunner);

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.in_flight = state.in_flight.saturating_sub(1);
    }
}

#[async_trait::async_trait]
impl CommandRunner for RecordingRunner {
    async fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
        let (delay, scripted) = {
            let mut state = self.state();
            state.invocations.push(invocation.clone());
            state.in_flight += 1;
            state.max_in_flight = state.max_in_flight.max(state.in_flight);
            // Popped at call start, so concurrent runs get scripted results in call order.
            (state.delay, state.outputs.pop_front())
        };
        let _guard = InFlight(self.clone());
        let argv0 = invocation.argv.first().ok_or(RunError::EmptyCommand)?;
        let output = match scripted {
            None => Output::success(),
            Some(Ok(output)) => output,
            // Spawn failures are immediate: no delay.
            Some(Err(message)) => {
                return Err(RunError::Spawn {
                    argv0: argv0.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, message),
                });
            }
        };
        if delay > invocation.timeout {
            tokio::time::sleep(invocation.timeout).await;
            return Ok(Output {
                status: CommandStatus::TimedOut(invocation.timeout),
                ..output
            });
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Ok(output)
    }

    fn spawn_detached(&self, argv: &[String]) -> Result<u32, RunError> {
        if argv.is_empty() {
            return Err(RunError::EmptyCommand);
        }
        let mut state = self.state();
        state.detached.push(argv.to_vec());
        let n = u32::try_from(state.detached.len() - 1).unwrap_or(u32::MAX - FAKE_PID_BASE);
        Ok(FAKE_PID_BASE + n)
    }
}
