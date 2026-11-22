//! Test-only consumer handoff. It closes and really joins the original writer
//! before starting the existing Stops backend over its exact retained core.
//! It is not a native/runtime join witness, role factory or production API.
use super::*;
use std::sync::Mutex;

pub(in crate::broker) struct StopsHandoffForTest(Arc<Handoff>);
struct Handoff {
    stops: StopJournal,
    _producer: Arc<retirement_owners::RetirementOwners>,
    capacity: u32,
    timeout: Duration,
    state: Mutex<State>,
}
enum State {
    Ready(std::thread::JoinHandle<()>),
    Joining,
    Complete(ProfileWriterCapabilities),
    Failed(JournalError),
}

impl WriterClient {
    /// On every pre-admission failure the original client remains in `input`.
    /// The returned guard owns the same STOP core before the last sender drops.
    #[cfg(test)]
    pub(in crate::broker) fn prepare_stops_handoff_for_test(
        input: &mut Option<WriterClient>,
    ) -> JournalResult<StopsHandoffForTest> {
        let borrowed = input.as_ref().ok_or(JournalError::InvalidInput)?;
        if Arc::strong_count(&borrowed.0) != 1 {
            return Err(JournalError::Busy);
        }
        let (stops, capacity) = borrowed.0.retirements.retained_stops_for_test()?;
        let original = input.take().ok_or(JournalError::InvalidInput)?;
        let state = match Arc::try_unwrap(original.0) {
            Ok(state) => state,
            Err(state) => {
                *input = Some(WriterClient(state));
                return Err(JournalError::Busy);
            }
        };
        let join = match state.retirements.take_writer_join_for_test() {
            Ok(join) => join,
            Err(error) => {
                *input = Some(WriterClient(Arc::new(state)));
                return Err(error);
            }
        };
        let retained = Arc::new(Handoff {
            stops,
            capacity,
            timeout: state.timeout,
            _producer: Arc::clone(&state.retirements),
            state: Mutex::new(State::Ready(join)),
        });
        // No Arc<ClientState> remains, so no facade/future can send late work.
        // The real OS join below also cannot succeed while any raw sender or
        // queue permit remains. No new actor is started before that real join.
        let ClientState {
            sender,
            scope: _,
            timeout: _,
            inventory_pending: _,
            retirements: _,
            status_cache: _,
            outstanding: _,
        } = state;
        drop(sender);
        Ok(StopsHandoffForTest(retained))
    }
}
impl StopsHandoffForTest {
    pub(in crate::broker) async fn joined_stops(&self) -> JournalResult<ProfileWriterCapabilities> {
        let join = {
            let mut state = self.0.state.lock().map_err(|_| JournalError::Corrupt)?;
            match std::mem::replace(&mut *state, State::Joining) {
                State::Ready(join) => join,
                State::Complete(capabilities) => {
                    let result = capabilities.clone();
                    *state = State::Complete(capabilities);
                    return Ok(result);
                }
                State::Failed(error) => {
                    *state = State::Failed(error);
                    return Err(error);
                }
                State::Joining => return Err(JournalError::Busy),
            }
        };
        let owner = Arc::clone(&self.0);
        let completed = tokio::task::spawn_blocking(move || {
            let result = (|| {
                join.join().map_err(|_| JournalError::Io)?;
                let actor = ClientState::start(
                    Backend::Stops(owner.stops.clone()),
                    owner.capacity,
                    owner.timeout,
                )?;
                Ok(ProfileWriterCapabilities {
                    full: None,
                    stops: StopWriterClient(Arc::clone(&actor)),
                    cold_audit: Some(ColdAuditWriterClient::from_stops_actor(actor)),
                })
            })();
            // Store the actual restricted actor/result before a notification
            // can be lost. This guard never contains the old sender/client.
            *owner.state.lock().unwrap_or_else(|e| e.into_inner()) = match result {
                Ok(capabilities) => State::Complete(capabilities),
                Err(error) => State::Failed(error),
            };
        })
        .await;
        if completed.is_err() {
            *self.0.state.lock().unwrap_or_else(|e| e.into_inner()) =
                State::Failed(JournalError::Io);
        }
        let state = self.0.state.lock().map_err(|_| JournalError::Corrupt)?;
        match &*state {
            State::Complete(capabilities) => Ok(capabilities.clone()),
            State::Failed(error) => Err(*error),
            _ => Err(JournalError::Interrupted),
        }
    }
}
