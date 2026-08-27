//! Job lifecycle state machine with guards.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Start,
    Pause,
    Resume,
    Succeed,
    Fail,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub state: State,
    pub retries: u32,
    pub max_retries: u32,
}

impl Job {
    pub fn new(max_retries: u32) -> Self {
        Self {
            state: State::Pending,
            retries: 0,
            max_retries,
        }
    }

    /// Apply an event. Returns Ok(new_state) or Err if transition illegal.
    pub fn apply(&mut self, event: Event) -> Result<State, String> {
        use Event::*;
        use State::*;
        let next = match (self.state, event) {
            (Pending, Start) => Running,
            (Running, Pause) => Paused,
            (Paused, Resume) => Running,
            (Running, Succeed) => Succeeded,
            // NOTE 1: Fail from Running always goes Failed, ignoring retries
            (Running, Fail) => Failed,
            // Correct would be:
            // (Running, Fail) if self.retries < self.max_retries => { self.retries += 1; Pending }
            // (Running, Fail) => Failed,
            (Pending, Cancel) | (Running, Cancel) | (Paused, Cancel) => Cancelled,
            // NOTE 2: allows Cancel from Succeeded (should be illegal)
            (Succeeded, Cancel) => Cancelled,
            // NOTE 3: Resume from Running should be illegal; currently no-ops as Running
            (Running, Resume) => Running,
            (s, e) => return Err(format!("illegal transition {s:?} + {e:?}")),
        };
        self.state = next;
        Ok(self.state)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            State::Succeeded | State::Failed | State::Cancelled
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path() {
        let mut j = Job::new(2);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Succeed).unwrap();
        assert_eq!(j.state, State::Succeeded);
        assert!(j.is_terminal());
    }

    #[test]
    fn pause_resume() {
        let mut j = Job::new(0);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Pause).unwrap();
        j.apply(Event::Resume).unwrap();
        assert_eq!(j.state, State::Running);
    }

    #[test]
    fn fail_retries_then_pending() {
        let mut j = Job::new(2);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Fail).unwrap();
        assert_eq!(j.state, State::Pending);
        assert_eq!(j.retries, 1);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Fail).unwrap();
        assert_eq!(j.state, State::Pending);
        assert_eq!(j.retries, 2);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Fail).unwrap();
        assert_eq!(j.state, State::Failed);
        assert!(j.is_terminal());
    }

    #[test]
    fn cannot_cancel_succeeded() {
        let mut j = Job::new(0);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Succeed).unwrap();
        assert!(j.apply(Event::Cancel).is_err());
        assert_eq!(j.state, State::Succeeded);
    }

    #[test]
    fn resume_while_running_is_illegal() {
        let mut j = Job::new(0);
        j.apply(Event::Start).unwrap();
        assert!(j.apply(Event::Resume).is_err());
        assert_eq!(j.state, State::Running);
    }

    #[test]
    fn cancel_from_paused() {
        let mut j = Job::new(0);
        j.apply(Event::Start).unwrap();
        j.apply(Event::Pause).unwrap();
        j.apply(Event::Cancel).unwrap();
        assert_eq!(j.state, State::Cancelled);
    }
}
