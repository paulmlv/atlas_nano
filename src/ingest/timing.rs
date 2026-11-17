//! Working time: from a user request to the model's recorded completion.

use std::collections::HashSet;

use time::UtcDateTime;

use crate::model::Turn;

/// What one record says about a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// A visible user request; `steer` marks one sent while the model worked.
    Request { steer: bool },
    /// The model or a tool worked.
    Activity,
    /// The model finished its reply.
    Complete,
    /// The reply was interrupted or failed.
    Interrupted,
}

/// One timing signal, in record order. `run` is the client's run ID, when it records one.
#[derive(Debug, Clone)]
pub struct Event {
    pub signal: Signal,
    pub at: Option<UtcDateTime>,
    pub run: Option<String>,
}

/// Pair each request with the completion recorded before the next request.
///
/// Working time is known only when the last terminal signal of the turn is a completion with no
/// work after it, at or after the request, and neither this request nor the next is a steer.
/// When both the request and the signal have a run ID, the IDs must match. A request without a
/// run ID can use a signal from a run with no recorded request.
pub fn turns(events: &[Event]) -> Vec<Turn> {
    let requests: Vec<usize> = (0..events.len())
        .filter(|&n| matches!(events[n].signal, Signal::Request { .. }))
        .collect();
    let request_runs: HashSet<&str> = requests
        .iter()
        .filter_map(|&n| events[n].run.as_deref())
        .collect();
    let mut turns = Vec::with_capacity(requests.len());
    for (index, &start) in requests.iter().enumerate() {
        let request = &events[start];
        let Some(at) = request.at else { continue };
        let end = requests.get(index + 1).copied().unwrap_or(events.len());
        let next_steer = events
            .get(end)
            .is_some_and(|e| e.signal == Signal::Request { steer: true });
        let steer = request.signal == Signal::Request { steer: true };
        let evidence = events[start + 1..end].iter().filter(|event| {
            match (request.run.as_deref(), event.run.as_deref()) {
                (Some(request_run), Some(run)) => request_run == run,
                (None, Some(run)) => !request_runs.contains(&run),
                (_, None) => true,
            }
        });
        let mut terminal: Option<&Event> = None;
        let mut work_after = false;
        for event in evidence {
            match event.signal {
                Signal::Complete | Signal::Interrupted => {
                    terminal = Some(event);
                    work_after = false;
                }
                Signal::Activity => work_after = terminal.is_some(),
                Signal::Request { .. } => {}
            }
        }
        let working = match terminal {
            Some(Event {
                signal: Signal::Complete,
                at: Some(done),
                ..
            }) if !steer && !next_steer && !work_after && *done >= at => Some(*done - at),
            _ => None,
        };
        turns.push(Turn { at, working });
    }
    turns
}

#[cfg(test)]
mod tests {
    use time::SignedDuration;

    use super::*;

    fn event(signal: Signal, second: i64) -> Event {
        Event {
            signal,
            at: Some(UtcDateTime::UNIX_EPOCH + SignedDuration::seconds(second)),
            run: None,
        }
    }

    const REQUEST: Signal = Signal::Request { steer: false };

    #[test]
    fn a_completed_turn_lasts_from_request_to_completion() {
        let events = [
            event(REQUEST, 0),
            event(Signal::Activity, 5),
            event(Signal::Complete, 9),
            event(REQUEST, 30),
        ];
        let turns = turns(&events);
        assert_eq!(turns[0].working, Some(SignedDuration::seconds(9)));
        assert_eq!(turns[1].working, None);
    }

    #[test]
    fn interrupted_steered_and_resumed_turns_are_unknown() {
        let interrupted = [event(REQUEST, 0), event(Signal::Interrupted, 4)];
        assert_eq!(turns(&interrupted)[0].working, None);
        let steered = [
            event(REQUEST, 0),
            event(Signal::Request { steer: true }, 2),
            event(Signal::Complete, 9),
        ];
        assert!(turns(&steered).iter().all(|turn| turn.working.is_none()));
        let resumed = [
            event(REQUEST, 0),
            event(Signal::Complete, 4),
            event(Signal::Activity, 6),
        ];
        assert_eq!(turns(&resumed)[0].working, None);
    }

    #[test]
    fn another_requests_run_is_not_evidence() {
        let mut first = event(REQUEST, 0);
        first.run = Some("a".into());
        let mut late = event(Signal::Complete, 3);
        late.run = Some("b".into());
        let mut second = event(REQUEST, 5);
        second.run = Some("b".into());
        let turns = turns(&[first, late, second]);
        assert_eq!(turns[0].working, None);
    }
}
