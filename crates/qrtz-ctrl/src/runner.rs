//! One guarded action (ADR 0214, amending ADR 0100 §4–§5). The runner never acts without the stop-key watcher:
//!
//! 1. the host's policy ([`check`]): the handle is in the tree or the menu, the element offers the action, the
//!    combo is the app's own or built-in, the address is http(s), the text is bounded;
//! 2. arm the watcher (`ctrl.observe {arm: true}`); a tripped or unarmed watcher ends the run before anything is
//!    sent (`stopped`);
//! 3. `ctrl.ax.act` (the helper re-checks the handle's `sig` and refuses once the watcher has tripped);
//! 4. read the watcher again: Esc or Jared's input during the action ends the run `stopped`.
//!
//! Everything lands in the log as ids, enums and numbers.
use crate::act::{combo_allowed, http_url, offered, Action, Refusal, TEXT_MAX};
use crate::ax::find;
use crate::chain::resolve;
use crate::guard::{verdict, StopReason};
use crate::helper::HelperClient;
use crate::log::{CtrlState, Event, EventLog, GuardPhase};

/// Monotonic and wall time; injected so tests are deterministic.
pub trait Clock: Send + Sync {
    fn mono_ns(&self) -> u64;
    fn wall_ms(&self) -> u64;
}

/// The host's policy over one action in the current state: the element's signature to send (if any) or why not.
pub fn check(a: &Action, s: &CtrlState, goal: &str) -> Result<Option<String>, Refusal> {
    let app = s.app.as_deref().unwrap_or_default();
    let browser = s
        .apps
        .as_ref()
        .is_some_and(|x| x.apps.iter().any(|y| y.id == app && y.browser));
    let menu = s.shortcuts.get(app).map(Vec::as_slice).unwrap_or_default();
    // A menu handle: pressing it is `press` on a handle the menu read produced.
    if let Action::Press { handle } = a {
        if menu.iter().any(|m| &m.id == handle)
            && s.tree.as_ref().is_none_or(|t| find(&t.root, handle).is_none())
        {
            return Ok(None);
        }
    }
    match a {
        Action::OpenApp { app } => {
            let known = s
                .apps
                .as_ref()
                .is_some_and(|x| x.apps.iter().any(|y| &y.id == app));
            known.then_some(None).ok_or(Refusal::UnknownApp)
        }
        Action::KeyCombo { combo, .. } => {
            let combos: Vec<String> = menu.iter().map(|m| m.combo.clone()).collect();
            combo_allowed(combo, &combos, browser)
                .then_some(None)
                .ok_or(Refusal::ComboNotAllowed)
        }
        Action::OpenUrl { url_ref, .. } => match resolve(url_ref, goal) {
            Some(u) if http_url(&u) && browser => Ok(None),
            _ => Err(Refusal::BadUrl),
        },
        _ => {
            let h = a.handle().ok_or(Refusal::Stale)?;
            let n = s
                .tree
                .as_ref()
                .and_then(|t| find(&t.root, h))
                .ok_or(Refusal::Stale)?;
            if !offered(a, n) {
                return Err(Refusal::NotOffered);
            }
            if let Some(r) = a.text_ref() {
                match resolve(r, goal) {
                    Some(t) if t.chars().count() <= TEXT_MAX => {}
                    _ => return Err(Refusal::BadText),
                }
            }
            Ok(Some(n.sig.clone()))
        }
    }
}

/// How one action ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// Acted (the log says whether the helper succeeded).
    Acted,
    /// The policy refused it.
    Refused,
    /// Esc, Jared's input, or no watcher: the run is over.
    Stopped(StopReason),
}

pub struct Runner<'a> {
    pub log: &'a mut EventLog,
    pub helper: &'a HelperClient,
    pub clock: &'a dyn Clock,
    pub goal: &'a str,
}

impl Runner<'_> {
    fn push(&mut self, e: Event) {
        let (m, w) = (self.clock.mono_ns(), self.clock.wall_ms());
        self.log.append(m, w, e);
    }

    fn guard(&mut self, action_id: &str, phase: GuardPhase, arm: Option<bool>) -> Option<StopReason> {
        let run = self.log.run_id().to_string();
        let o = self.helper.observe(&run, arm).unwrap_or_default();
        let stop = verdict(&o);
        self.push(Event::GuardResult {
            action_id: action_id.into(),
            phase,
            stop,
            frontmost: o.frontmost,
        });
        stop
    }

    pub fn run(&mut self, action_id: &str, action: &Action) -> Ran {
        let st = self.log.state();
        let sig = match check(action, &st, self.goal) {
            Ok(s) => s,
            Err(r) => {
                self.push(Event::ActionRequested {
                    action_id: action_id.into(),
                    action: action.clone(),
                });
                self.push(Event::ActionRefused {
                    action_id: action_id.into(),
                    code: r.code().into(),
                });
                return Ran::Refused;
            }
        };
        if let Some(stop) = self.guard(action_id, GuardPhase::Before, Some(true)) {
            return Ran::Stopped(stop);
        }
        self.push(Event::ActionRequested {
            action_id: action_id.into(),
            action: action.clone(),
        });
        let text = action.text_ref().and_then(|r| resolve(r, self.goal));
        let t0 = self.clock.mono_ns();
        let run = self.log.run_id().to_string();
        let res = self
            .helper
            .act(&run, action.wire(text.as_deref()), sig.as_deref());
        let elapsed_ms = self.clock.mono_ns().saturating_sub(t0) / 1_000_000;
        // A helper that refused because its watcher tripped is a stop, not a failed action.
        let stopped_by_helper = matches!(&res, Err(c) if c == "guard.stopped");
        self.push(Event::ActionExecuted {
            action_id: action_id.into(),
            ok: res.is_ok(),
            err: res.err(),
            elapsed_ms,
        });
        match self.guard(action_id, GuardPhase::After, None) {
            Some(stop) => Ran::Stopped(stop),
            None if stopped_by_helper => Ran::Stopped(StopReason::StopKey),
            None => Ran::Acted,
        }
    }
}
