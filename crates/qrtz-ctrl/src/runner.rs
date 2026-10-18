//! The execution effect (ADR 0100): decide on the ladder, snapshot what the user owns, execute, snapshot again,
//! record everything in the log. The runner never retries a route that violated the guard: the violation
//! quarantines that (app, rung) for the rest of the run, through the log-derived state.
use crate::guard::{compare, Field, UiSnapshot};
use crate::ladder::{choose, Decision, Env, Route};
use crate::log::{Event, EventLog, Phase};

/// Reads what the user owns. On macOS: `NSWorkspace.frontmostApplication`, `CGEvent(source: nil).location`, the
/// system-wide `AXFocusedUIElement`, the key window, and HID idle time.
pub trait UiProbe {
    fn snapshot(&self) -> UiSnapshot;
}

/// Performs one route. Errors are short codes, never content.
pub trait Executor {
    fn execute(&mut self, route: &Route) -> Result<(), String>;
    /// Put back what an action changed (re-activate the previously frontmost app). Returns whether it worked.
    /// The default does nothing: a violation is still recorded either way.
    fn restore(&mut self, _before: &UiSnapshot, _fields: &[Field]) -> bool {
        false
    }
}

/// Monotonic and wall time; injected so tests are deterministic.
pub trait Clock {
    fn mono_ns(&self) -> u64;
    fn wall_ms(&self) -> u64;
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Done { clean: bool },
    Failed { err: String, clean: bool },
    Deferred { retry_after_ms: u64 },
    Asked,
    Refused,
}

pub struct Runner<'a, P: UiProbe, E: Executor, C: Clock> {
    pub log: &'a mut EventLog,
    pub probe: &'a P,
    pub exec: &'a mut E,
    pub clock: &'a C,
}

impl<P: UiProbe, E: Executor, C: Clock> Runner<'_, P, E, C> {
    fn push(&mut self, e: Event) {
        let (m, w) = (self.clock.mono_ns(), self.clock.wall_ms());
        self.log.append(m, w, e);
    }

    /// Run one action. `env` comes from the helper's probes; its quarantine set is replaced by the log's.
    pub fn run(&mut self, action_id: &str, routes: &[Route], env: &Env) -> RunOutcome {
        let mut env = env.clone();
        env.quarantined = self.log.state().quarantined;
        self.push(Event::ActionRequested {
            action_id: action_id.into(),
            routes: routes.to_vec(),
        });
        let (decision, verdicts) = choose(routes, &env);
        self.push(Event::ActionDecided {
            action_id: action_id.into(),
            decision: decision.clone(),
            verdicts,
        });
        let (index, rung) = match decision {
            Decision::Execute { index, rung } => (index, rung),
            Decision::Defer { retry_after_ms } => return RunOutcome::Deferred { retry_after_ms },
            Decision::Ask { .. } => return RunOutcome::Asked,
            Decision::Refuse => return RunOutcome::Refused,
        };
        let route = &routes[index];
        let before = self.probe.snapshot();
        self.push(Event::GuardSnapshot {
            action_id: action_id.into(),
            phase: Phase::Before,
            snapshot: before.clone(),
        });
        let t0 = self.clock.mono_ns();
        let result = self.exec.execute(route);
        let elapsed_ms = self.clock.mono_ns().saturating_sub(t0) / 1_000_000;
        let after = self.probe.snapshot();
        self.push(Event::GuardSnapshot {
            action_id: action_id.into(),
            phase: Phase::After,
            snapshot: after.clone(),
        });
        let outcome = compare(&before, &after, elapsed_ms);
        let clean = outcome.clean();
        let violated = outcome.violations.clone();
        self.push(Event::ActionExecuted {
            action_id: action_id.into(),
            ok: result.is_ok(),
            err: result.as_ref().err().cloned(),
            elapsed_ms,
        });
        self.push(Event::GuardResult {
            action_id: action_id.into(),
            app: route.app().map(str::to_string),
            rung,
            outcome,
        });
        if !clean {
            let ok = self.exec.restore(&before, &violated);
            self.push(Event::GuardRestored {
                action_id: action_id.into(),
                fields: violated,
                ok,
            });
        }
        match result {
            Ok(()) => RunOutcome::Done { clean },
            Err(err) => RunOutcome::Failed { err, clean },
        }
    }
}
