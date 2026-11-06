//! Private bounded contexts and fixed-size standard AEAD cursor envelopes.
use crate::identity::{ContextModeV1, MembershipV1, random_bytes};
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) const CONTEXT_TTL: Duration = Duration::from_secs(300);
pub(super) const MAX_CONTEXTS: usize = 4096;
pub(super) const MAX_OWNER_CONTEXTS: usize = 64;
const PADDED_BYTES: usize = 512;
const NONCE_BYTES: usize = 12;
pub(super) const CURSOR_HEX_BYTES: usize = crate::identity::HUMAN_CURSOR_HEX_BYTES;
const _: () = assert!(CURSOR_HEX_BYTES == (1 + NONCE_BYTES + PADDED_BYTES + 16) * 2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StateFailure {
    InvalidCursor,
    InvalidContext,
    Expired,
    Revoked,
    Capacity,
    Unavailable,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub(super) struct ContextBinding {
    pub digest: [u8; 32],
    pub principal_key: String,
    pub credential_id: String,
    pub credential_revision: u64,
    pub authentication_method: String,
    pub mapping_revision: Option<String>,
    pub tenant_id: String,
    pub organization_scope: String,
    pub membership_id: String,
    pub mode: ContextModeV1,
    pub membership_revision: u64,
    pub authorization_revision: u64,
}

pub(super) struct RegisteredContext {
    pub id: String,
    pub authorization_version: String,
    pub binding: ContextBinding,
    pub expires_at: String,
    created: Instant,
    lifetime: Duration,
    active: AtomicBool,
    cipher: CursorCipher,
    key_fingerprint: [u8; 32],
}
impl RegisteredContext {
    pub fn ensure_valid(&self) -> Result<(), StateFailure> {
        if self.created.elapsed() >= self.lifetime {
            return Err(StateFailure::Expired);
        }
        if !self.active.load(Ordering::Acquire) {
            return Err(StateFailure::Revoked);
        }
        Ok(())
    }
    pub fn elapsed_millis(&self) -> Result<u64, StateFailure> {
        self.ensure_valid()?;
        u64::try_from(self.created.elapsed().as_millis()).map_err(|_| StateFailure::Unavailable)
    }
    fn deadline_millis(&self) -> Result<u64, StateFailure> {
        u64::try_from(self.lifetime.as_millis()).map_err(|_| StateFailure::Unavailable)
    }
    fn aad<Q: Serialize>(&self, query: &Q) -> Result<Vec<u8>, StateFailure> {
        let mut bytes = b"ai-guard:human-cursor:chacha20poly1305:v2\0".to_vec();
        bytes.extend_from_slice(self.id.as_bytes());
        bytes.extend_from_slice(self.authorization_version.as_bytes());
        bytes.extend_from_slice(
            &serde_json::to_vec(&(&self.binding, query)).map_err(|_| StateFailure::Unavailable)?,
        );
        Ok(bytes)
    }
    pub fn start<Q: Serialize>(
        &self,
        query: &Q,
        token: Option<&str>,
    ) -> Result<CursorState, StateFailure> {
        self.ensure_valid()?;
        let state = match token {
            Some(token) => self.cipher.open(&self.aad(query)?, token)?,
            None => CursorState {
                version: 2,
                after_resource: String::new(),
                after_event: 0,
                upper_event: None,
                pages: 0,
                bytes: 0,
                started_millis: self.elapsed_millis()?,
                deadline_millis: self.deadline_millis()?,
            },
        };
        self.check_state(&state)?;
        Ok(state)
    }
    fn check_state(&self, state: &CursorState) -> Result<(), StateFailure> {
        if state.version != 2
            || state.after_event < 0
            || state
                .upper_event
                .is_some_and(|upper| upper < state.after_event)
            || (!state.after_resource.is_empty() && !hex_64(&state.after_resource))
        {
            return Err(StateFailure::InvalidCursor);
        }
        self.check_budget(
            state.pages,
            state.bytes,
            state.started_millis,
            state.deadline_millis,
        )
    }
    pub fn advance(
        &self,
        mut state: CursorState,
        bytes: usize,
    ) -> Result<CursorState, StateFailure> {
        self.check_state(&state)?;
        add_budget(&mut state.pages, &mut state.bytes, bytes)?;
        Ok(state)
    }
    pub fn seal<Q: Serialize>(
        &self,
        query: &Q,
        state: &CursorState,
    ) -> Result<String, StateFailure> {
        self.check_state(state)?;
        self.cipher.seal(&self.aad(query)?, state)
    }
    fn check_budget(
        &self,
        pages: u32,
        bytes: u64,
        started: u64,
        deadline: u64,
    ) -> Result<(), StateFailure> {
        if pages > 1000
            || bytes > 16 * 1024 * 1024
            || deadline != self.deadline_millis()?
            || started > deadline
        {
            return Err(StateFailure::InvalidCursor);
        }
        if self.elapsed_millis()? >= deadline {
            return Err(StateFailure::Expired);
        }
        Ok(())
    }
    fn broker_aad<Q: Serialize>(&self, query: &Q) -> Result<Vec<u8>, StateFailure> {
        self.aad(&("human_broker_registry_live_keyset_v1", query))
    }
    pub fn start_brokers<Q: Serialize>(
        &self,
        query: &Q,
        token: Option<&str>,
    ) -> Result<BrokerCursorStateV1, StateFailure> {
        self.ensure_valid()?;
        let state = match token {
            Some(token) => self.cipher.open_value(&self.broker_aad(query)?, token)?,
            None => BrokerCursorStateV1 {
                version: 1,
                after_broker_id: None,
                pages: 0,
                bytes: 0,
                started_millis: self.elapsed_millis()?,
                deadline_millis: self.deadline_millis()?,
            },
        };
        self.check_broker_state(&state)?;
        Ok(state)
    }
    fn check_broker_state(&self, state: &BrokerCursorStateV1) -> Result<(), StateFailure> {
        if state.version != 1
            || state.after_broker_id.as_ref().is_some_and(|value| {
                !uuid::Uuid::parse_str(value)
                    .is_ok_and(|id| !id.is_nil() && id.to_string() == *value)
            })
        {
            return Err(StateFailure::InvalidCursor);
        }
        self.check_budget(
            state.pages,
            state.bytes,
            state.started_millis,
            state.deadline_millis,
        )
    }
    pub fn advance_brokers(
        &self,
        mut state: BrokerCursorStateV1,
        bytes: usize,
    ) -> Result<BrokerCursorStateV1, StateFailure> {
        self.check_broker_state(&state)?;
        add_budget(&mut state.pages, &mut state.bytes, bytes)?;
        Ok(state)
    }
    pub fn seal_brokers<Q: Serialize>(
        &self,
        query: &Q,
        state: &BrokerCursorStateV1,
    ) -> Result<String, StateFailure> {
        self.check_broker_state(state)?;
        self.cipher.seal(&self.broker_aad(query)?, state)
    }
    fn worker_aad<Q: Serialize>(&self, query: &Q) -> Result<Vec<u8>, StateFailure> {
        self.aad(&("human_worker_registry_live_keyset_v1", query))
    }
    pub fn start_workers<Q: Serialize>(
        &self,
        query: &Q,
        token: Option<&str>,
    ) -> Result<WorkerCursorStateV1, StateFailure> {
        self.ensure_valid()?;
        let state = match token {
            Some(token) => self.cipher.open_value(&self.worker_aad(query)?, token)?,
            None => WorkerCursorStateV1 {
                version: 1,
                after_worker_id: None,
                pages: 0,
                bytes: 0,
                started_millis: self.elapsed_millis()?,
                deadline_millis: self.deadline_millis()?,
            },
        };
        self.check_worker_state(&state)?;
        Ok(state)
    }
    fn check_worker_state(&self, state: &WorkerCursorStateV1) -> Result<(), StateFailure> {
        if state.version != 1
            || state.after_worker_id.as_ref().is_some_and(|value| {
                !uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == *value)
            })
        {
            return Err(StateFailure::InvalidCursor);
        }
        self.check_budget(
            state.pages,
            state.bytes,
            state.started_millis,
            state.deadline_millis,
        )
    }
    pub fn advance_workers(
        &self,
        mut state: WorkerCursorStateV1,
        bytes: usize,
    ) -> Result<WorkerCursorStateV1, StateFailure> {
        self.check_worker_state(&state)?;
        add_budget(&mut state.pages, &mut state.bytes, bytes)?;
        Ok(state)
    }
    pub fn seal_workers<Q: Serialize>(
        &self,
        query: &Q,
        state: &WorkerCursorStateV1,
    ) -> Result<String, StateFailure> {
        self.check_worker_state(state)?;
        self.cipher.seal(&self.worker_aad(query)?, state)
    }
}
fn add_budget(pages: &mut u32, total: &mut u64, bytes: usize) -> Result<(), StateFailure> {
    *pages = pages.checked_add(1).ok_or(StateFailure::Capacity)?;
    *total = total
        .checked_add(u64::try_from(bytes).map_err(|_| StateFailure::Capacity)?)
        .ok_or(StateFailure::Capacity)?;
    if *pages > 1000 || *total > 16 * 1024 * 1024 {
        return Err(StateFailure::Capacity);
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrokerCursorStateV1 {
    version: u32,
    pub after_broker_id: Option<String>,
    pages: u32,
    bytes: u64,
    started_millis: u64,
    deadline_millis: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerCursorStateV1 {
    version: u32,
    pub after_worker_id: Option<String>,
    pages: u32,
    bytes: u64,
    started_millis: u64,
    deadline_millis: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CursorState {
    version: u32,
    pub after_resource: String,
    pub after_event: i64,
    pub upper_event: Option<i64>,
    pages: u32,
    bytes: u64,
    started_millis: u64,
    deadline_millis: u64,
}

struct CursorCipher {
    key: LessSafeKey,
    next: AtomicU64,
}
impl CursorCipher {
    fn new(bytes: &[u8; 32]) -> Result<Self, StateFailure> {
        let key =
            UnboundKey::new(&CHACHA20_POLY1305, bytes).map_err(|_| StateFailure::Unavailable)?;
        Ok(Self {
            key: LessSafeKey::new(key),
            next: AtomicU64::new(0),
        })
    }
    fn reserve(&self) -> Result<[u8; NONCE_BYTES], StateFailure> {
        // The terminal value never wraps and is deliberately not used. A
        // reserved nonce is consumed even if encoding or sealing later fails.
        let value = self
            .next
            .try_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                next.checked_add(1)
            })
            .map_err(|_| StateFailure::Unavailable)?;
        let mut nonce = [0u8; NONCE_BYTES];
        nonce[4..].copy_from_slice(&value.to_be_bytes());
        Ok(nonce)
    }
    fn seal<T: Serialize>(&self, aad: &[u8], state: &T) -> Result<String, StateFailure> {
        let nonce = self.reserve()?;
        let json = serde_json::to_vec(state).map_err(|_| StateFailure::Unavailable)?;
        if json.len() > PADDED_BYTES - 2 {
            return Err(StateFailure::Unavailable);
        }
        let mut ciphertext = vec![0u8; PADDED_BYTES];
        ciphertext[..2].copy_from_slice(
            &u16::try_from(json.len())
                .map_err(|_| StateFailure::Unavailable)?
                .to_be_bytes(),
        );
        ciphertext[2..2 + json.len()].copy_from_slice(&json);
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut ciphertext,
            )
            .map_err(|_| StateFailure::Unavailable)?;
        let mut wire = Vec::with_capacity(CURSOR_HEX_BYTES / 2);
        wire.push(2);
        wire.extend_from_slice(&nonce);
        wire.extend_from_slice(&ciphertext);
        Ok(hex::encode(wire))
    }
    fn open(&self, aad: &[u8], token: &str) -> Result<CursorState, StateFailure> {
        self.open_value(aad, token)
    }
    fn open_value<T: serde::de::DeserializeOwned>(
        &self,
        aad: &[u8],
        token: &str,
    ) -> Result<T, StateFailure> {
        // Authentic existing tokens may be opened repeatedly. Only sealing
        // reserves nonces; no encryption accepts a caller-supplied nonce.
        if token.len() != CURSOR_HEX_BYTES || !token.bytes().all(lower_hex) {
            return Err(StateFailure::InvalidCursor);
        }
        let wire = hex::decode(token).map_err(|_| StateFailure::InvalidCursor)?;
        if wire[0] != 2 || wire[1..5] != [0; 4] {
            return Err(StateFailure::InvalidCursor);
        }
        let nonce: [u8; NONCE_BYTES] = wire[1..1 + NONCE_BYTES]
            .try_into()
            .map_err(|_| StateFailure::InvalidCursor)?;
        let mut ciphertext = wire[1 + NONCE_BYTES..].to_vec();
        let plaintext = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut ciphertext,
            )
            .map_err(|_| StateFailure::InvalidCursor)?;
        if plaintext.len() != PADDED_BYTES {
            return Err(StateFailure::InvalidCursor);
        }
        let size = usize::from(u16::from_be_bytes([plaintext[0], plaintext[1]]));
        if size > PADDED_BYTES - 2 || plaintext[2 + size..].iter().any(|byte| *byte != 0) {
            return Err(StateFailure::InvalidCursor);
        }
        serde_json::from_slice(&plaintext[2..2 + size]).map_err(|_| StateFailure::InvalidCursor)
    }
}

pub(super) struct ContextRegistry {
    entries: Mutex<HashMap<String, Arc<RegisteredContext>>>,
}
impl ContextRegistry {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
    fn purge_expired(entries: &mut HashMap<String, Arc<RegisteredContext>>) {
        entries.retain(|_, entry| {
            let valid =
                entry.created.elapsed() < entry.lifetime && entry.active.load(Ordering::Acquire);
            if !valid {
                entry.active.store(false, Ordering::Release);
            }
            valid
        });
    }
    pub fn reconcile(
        &self,
        digest: &[u8; 32],
        principal_key: &str,
        credential_revision: u64,
        tenant_id: &str,
        revision: Option<u64>,
        members: &[MembershipV1],
    ) -> Result<(), StateFailure> {
        let mut entries = self.entries.lock().map_err(|_| StateFailure::Unavailable)?;
        Self::purge_expired(&mut entries);
        entries.retain(|_, entry| {
            let binding = &entry.binding;
            if &binding.digest != digest || binding.principal_key != principal_key {
                return true;
            }
            let revoked_credential = binding.credential_revision < credential_revision;
            let revoked_scope = binding.tenant_id == tenant_id
                && revision.is_some_and(|revision| {
                    if binding.authorization_revision > revision {
                        return false;
                    }
                    binding.authorization_revision < revision
                        || !members.iter().any(|member| {
                            member.membership_id == binding.membership_id
                                && member.organization_scope == binding.organization_scope
                                && member.revision == binding.membership_revision
                                && member.active
                                && (binding.mode == ContextModeV1::Personal
                                    || member
                                        .capabilities
                                        .iter()
                                        .any(|cap| cap.is_administration()))
                        })
                });
            if revoked_credential || revoked_scope {
                entry.active.store(false, Ordering::Release);
                false
            } else {
                true
            }
        });
        Ok(())
    }
    pub fn obtain(
        &self,
        binding: ContextBinding,
        lifetime: Duration,
        expires_at: String,
    ) -> Result<Arc<RegisteredContext>, StateFailure> {
        let mut entries = self.entries.lock().map_err(|_| StateFailure::Unavailable)?;
        Self::purge_expired(&mut entries);
        if let Some(entry) = entries.values().find(|entry| entry.binding == binding) {
            entry.ensure_valid()?;
            return Ok(Arc::clone(entry));
        }
        if entries.values().any(|entry| {
            entry.binding.digest == binding.digest
                && entry.binding.principal_key == binding.principal_key
                && (entry.binding.credential_revision > binding.credential_revision
                    || (entry.binding.tenant_id == binding.tenant_id
                        && entry.binding.authorization_revision > binding.authorization_revision))
        }) {
            return Err(StateFailure::Revoked);
        }
        let own = entries
            .values()
            .filter(|entry| {
                entry.binding.digest == binding.digest
                    || entry.binding.principal_key == binding.principal_key
            })
            .count();
        if entries.len() >= MAX_CONTEXTS || own >= MAX_OWNER_CONTEXTS {
            return Err(StateFailure::Capacity);
        }
        let lifetime = lifetime.min(CONTEXT_TTL);
        if lifetime.is_zero() {
            return Err(StateFailure::Expired);
        }
        let id = random_bytes().map_err(|_| StateFailure::Unavailable)?;
        let version = random_bytes().map_err(|_| StateFailure::Unavailable)?;
        let key = random_bytes().map_err(|_| StateFailure::Unavailable)?;
        let fingerprint: [u8; 32] = Sha256::digest(key).into();
        let id_hex = hex::encode(id);
        if id == version
            || id == key
            || version == key
            || entries.contains_key(&id_hex)
            || entries
                .values()
                .any(|entry| entry.key_fingerprint == fingerprint)
        {
            return Err(StateFailure::Unavailable);
        }
        let entry = Arc::new(RegisteredContext {
            id: id_hex.clone(),
            authorization_version: hex::encode(version),
            binding,
            expires_at,
            created: Instant::now(),
            lifetime,
            active: AtomicBool::new(true),
            cipher: CursorCipher::new(&key)?,
            key_fingerprint: fingerprint,
        });
        entries.insert(id_hex, Arc::clone(&entry));
        Ok(entry)
    }
    pub fn lookup(
        &self,
        id: &str,
        digest: &[u8; 32],
        principal_key: &str,
        credential_id: &str,
        credential_revision: u64,
    ) -> Result<Arc<RegisteredContext>, StateFailure> {
        if !hex_64(id) {
            return Err(StateFailure::InvalidContext);
        }
        let entries = self.entries.lock().map_err(|_| StateFailure::Unavailable)?;
        let entry = entries.get(id).ok_or(StateFailure::InvalidContext)?;
        entry.ensure_valid()?;
        let binding = &entry.binding;
        if &binding.digest != digest
            || binding.principal_key != principal_key
            || binding.credential_id != credential_id
        {
            return Err(StateFailure::InvalidContext);
        }
        if binding.credential_revision != credential_revision {
            return Err(StateFailure::Revoked);
        }
        Ok(Arc::clone(entry))
    }
}
fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}
fn hex_64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(lower_hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> CursorState {
        CursorState {
            version: 2,
            after_resource: String::new(),
            after_event: 12,
            upper_event: Some(99),
            pages: 1,
            bytes: 200,
            started_millis: 0,
            deadline_millis: 300000,
        }
    }
    fn binding() -> ContextBinding {
        ContextBinding {
            digest: [7; 32],
            principal_key: "a".repeat(64),
            credential_id: "87a54f19-3632-4df3-b676-039ba10f77ed".into(),
            credential_revision: 1,
            authentication_method: "local_credential".into(),
            mapping_revision: None,
            tenant_id: "tenant-a".into(),
            organization_scope: "org".into(),
            membership_id: "3bab416c-097d-40e2-b3ce-0f17122a83d8".into(),
            mode: ContextModeV1::Personal,
            membership_revision: 1,
            authorization_revision: 3,
        }
    }
    fn cipher_at(next: u64) -> CursorCipher {
        let mut cipher = CursorCipher::new(&[1; 32]).unwrap();
        cipher.next = AtomicU64::new(next);
        cipher
    }
    #[test]
    fn concurrent_seals_share_one_non_resetting_nonce_sequence() {
        let cipher = Arc::new(cipher_at(0));
        let tokens = std::thread::scope(|scope| {
            let handles = (0..32)
                .map(|_| {
                    let cipher = Arc::clone(&cipher);
                    scope.spawn(move || cipher.seal(b"bound", &state()).unwrap())
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let mut nonces = std::collections::BTreeSet::new();
        for token in tokens {
            assert_eq!(token.len(), CURSOR_HEX_BYTES);
            let bytes = hex::decode(&token).unwrap();
            assert!(nonces.insert(bytes[1..13].to_vec()));
            assert_eq!(cipher.open(b"bound", &token).unwrap().after_event, 12);
        }
        assert_eq!(nonces.len(), 32);
    }
    #[test]
    fn failed_encoding_consumes_nonce_and_exhaustion_never_wraps() {
        let cipher = cipher_at(0);
        let mut invalid = state();
        invalid.after_resource = "x".repeat(1024);
        assert!(matches!(
            cipher.seal(b"bound", &invalid),
            Err(StateFailure::Unavailable)
        ));
        let token = cipher.seal(b"bound", &state()).unwrap();
        let bytes = hex::decode(token).unwrap();
        assert_eq!(&bytes[5..13], &1u64.to_be_bytes());
        let terminal = cipher_at(u64::MAX - 1);
        assert!(terminal.seal(b"bound", &state()).is_ok());
        assert!(matches!(
            terminal.seal(b"bound", &state()),
            Err(StateFailure::Unavailable)
        ));
        assert!(matches!(
            terminal.seal(b"bound", &state()),
            Err(StateFailure::Unavailable)
        ));
    }
    #[test]
    fn fixed_padded_cursor_rejects_binding_key_format_and_tag_changes() {
        let cipher = cipher_at(0);
        let token = cipher.seal(b"query-a", &state()).unwrap();
        assert!(matches!(
            cipher.open(b"query-b", &token),
            Err(StateFailure::InvalidCursor)
        ));
        let other = CursorCipher::new(&[2; 32]).unwrap();
        assert!(matches!(
            other.open(b"query-a", &token),
            Err(StateFailure::InvalidCursor)
        ));
        let mut changed = hex::decode(&token).unwrap();
        *changed.last_mut().unwrap() ^= 1;
        assert!(matches!(
            cipher.open(b"query-a", &hex::encode(changed)),
            Err(StateFailure::InvalidCursor)
        ));
        assert!(matches!(
            cipher.open(b"query-a", "bad"),
            Err(StateFailure::InvalidCursor)
        ));
        let mut longer = state();
        longer.after_event = i64::MAX;
        longer.upper_event = Some(i64::MAX);
        assert_eq!(cipher.seal(b"query-a", &longer).unwrap().len(), token.len());
    }
    #[test]
    fn unchanged_context_reuses_arc_key_ttl_and_sequence_but_restart_rejects_it() {
        let registry = ContextRegistry::new();
        let entry = registry
            .obtain(binding(), CONTEXT_TTL, "expiry".into())
            .unwrap();
        let first = entry.seal(&"q", &state()).unwrap();
        let again = registry
            .obtain(binding(), CONTEXT_TTL, "later-expiry".into())
            .unwrap();
        assert!(Arc::ptr_eq(&entry, &again));
        assert_eq!(again.expires_at, "expiry");
        let second = again.seal(&"q", &state()).unwrap();
        assert_ne!(first, second);
        let restarted = ContextRegistry::new();
        assert!(matches!(
            restarted.lookup(
                &entry.id,
                &[7; 32],
                &"a".repeat(64),
                "87a54f19-3632-4df3-b676-039ba10f77ed",
                1
            ),
            Err(StateFailure::InvalidContext)
        ));
    }
    #[test]
    fn expiration_invalidates_held_arc_without_recreating_its_identifier() {
        let registry = ContextRegistry::new();
        let mut entry = registry
            .obtain(binding(), CONTEXT_TTL, "expiry".into())
            .unwrap();
        let valid_state = entry.start(&"q", None).unwrap();
        let previous_token = entry.seal(&"q", &valid_state).unwrap();
        let id = entry.id.clone();
        registry.entries.lock().unwrap().remove(&id);
        let unique = Arc::get_mut(&mut entry).unwrap();
        // Expire elapsed time while preserving the authentic state's fixed
        // deadline. Changing lifetime instead creates a different, invalid
        // configuration and exercises InvalidCursor before expiration.
        unique.created = unique.created.checked_sub(CONTEXT_TTL).unwrap();
        registry
            .entries
            .lock()
            .unwrap()
            .insert(id.clone(), Arc::clone(&entry));
        assert!(matches!(
            registry.lookup(
                &id,
                &[7; 32],
                &"a".repeat(64),
                "87a54f19-3632-4df3-b676-039ba10f77ed",
                1
            ),
            Err(StateFailure::Expired)
        ));
        assert!(matches!(
            entry.seal(&"q", &valid_state),
            Err(StateFailure::Expired)
        ));
        assert!(matches!(
            entry.start(&"q", Some(&previous_token)),
            Err(StateFailure::Expired)
        ));
        assert!(matches!(
            entry.advance(valid_state, 1),
            Err(StateFailure::Expired)
        ));
        let fresh = registry
            .obtain(binding(), CONTEXT_TTL, "new-expiry".into())
            .unwrap();
        assert_ne!(fresh.id, id);
        assert!(!Arc::ptr_eq(&entry, &fresh));
    }
    #[test]
    fn context_capacity_never_evicts_live_entries_or_resets_a_reused_context() {
        let registry = ContextRegistry::new();
        let first = registry
            .obtain(binding(), CONTEXT_TTL, "expiry".into())
            .unwrap();
        for number in 1..64 {
            let mut next = binding();
            next.membership_id = uuid::Uuid::from_u128(number).to_string();
            registry.obtain(next, CONTEXT_TTL, "expiry".into()).unwrap();
        }
        let mut overflow = binding();
        overflow.membership_id = uuid::Uuid::from_u128(9999).to_string();
        assert!(matches!(
            registry.obtain(overflow, CONTEXT_TTL, "expiry".into()),
            Err(StateFailure::Capacity)
        ));
        assert!(first.ensure_valid().is_ok());
        assert!(Arc::ptr_eq(
            &first,
            &registry
                .obtain(binding(), CONTEXT_TTL, "later".into())
                .unwrap()
        ));
        let mut foreign = binding();
        foreign.digest = [8; 32];
        foreign.principal_key = "b".repeat(64);
        assert!(
            registry
                .obtain(foreign, CONTEXT_TTL, "expiry".into())
                .is_ok()
        );
        assert!(
            registry
                .lookup(
                    &first.id,
                    &[7; 32],
                    &"a".repeat(64),
                    "87a54f19-3632-4df3-b676-039ba10f77ed",
                    1
                )
                .is_ok()
        );
    }
    #[test]
    fn authoritative_revocation_retires_held_arc_and_stale_snapshot_cannot_evict_newer_context() {
        use crate::identity::{HumanCapabilityV1, PrincipalRefV1};
        let registry = ContextRegistry::new();
        let old = registry
            .obtain(binding(), CONTEXT_TTL, "expiry".into())
            .unwrap();
        let member = MembershipV1 {
            version: 1,
            membership_id: binding().membership_id,
            principal: PrincipalRefV1::HumanLocal {
                installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
                account_id: "871be6f3-b1a5-4eb9-9c72-989cedb05c43".into(),
            },
            tenant_id: "tenant-a".into(),
            organization_scope: "org".into(),
            revision: 2,
            active: true,
            capabilities: std::collections::BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
        };
        registry
            .reconcile(
                &[7; 32],
                &"a".repeat(64),
                1,
                "tenant-a",
                Some(4),
                std::slice::from_ref(&member),
            )
            .unwrap();
        assert!(matches!(
            old.seal(&"q", &state()),
            Err(StateFailure::Revoked)
        ));
        let mut next = binding();
        next.authorization_revision = 4;
        next.membership_revision = 2;
        let current = registry.obtain(next, CONTEXT_TTL, "expiry".into()).unwrap();
        registry
            .reconcile(&[7; 32], &"a".repeat(64), 1, "tenant-a", Some(3), &[])
            .unwrap();
        assert!(current.ensure_valid().is_ok());
        assert!(matches!(
            registry.obtain(binding(), CONTEXT_TTL, "expiry".into()),
            Err(StateFailure::Revoked)
        ));
        assert_ne!(current.id, old.id);
        assert_ne!(current.authorization_version, old.authorization_version);
        assert!(
            current.key_fingerprint != old.key_fingerprint,
            "reacquired context key must be fresh"
        );
    }

    #[test]
    fn broker_and_legacy_cursors_share_nonce_and_reject_domains_positions_and_budget_overflow() {
        let registry = ContextRegistry::new();
        let mut entry = registry
            .obtain(binding(), CONTEXT_TTL, "expiry".into())
            .unwrap();
        let resource = entry.start(&"same-query", None).unwrap();
        let broker = entry.start_brokers(&"same-query", None).unwrap();
        let legacy_token = entry.seal(&"same-query", &resource).unwrap();
        let broker_token = entry.seal_brokers(&"same-query", &broker).unwrap();
        let second_legacy = entry.seal(&"same-query", &resource).unwrap();
        for (index, token) in [&legacy_token, &broker_token, &second_legacy]
            .into_iter()
            .enumerate()
        {
            let wire = hex::decode(token).unwrap();
            assert_eq!(wire.len() * 2, CURSOR_HEX_BYTES);
            assert_eq!(&wire[5..13], &u64::try_from(index).unwrap().to_be_bytes());
        }
        assert!(matches!(
            entry.start(&"same-query", Some(&broker_token)),
            Err(StateFailure::InvalidCursor)
        ));
        assert!(matches!(
            entry.start_brokers(&"same-query", Some(&legacy_token)),
            Err(StateFailure::InvalidCursor)
        ));
        assert!(matches!(
            entry.start_brokers(&"other-limit", Some(&broker_token)),
            Err(StateFailure::InvalidCursor)
        ));
        let mut position = broker.clone();
        position.after_broker_id = Some("a".repeat(64));
        assert!(matches!(
            entry.seal_brokers(&"same-query", &position),
            Err(StateFailure::InvalidCursor)
        ));
        let mut exhausted = broker.clone();
        exhausted.pages = 1000;
        assert!(matches!(
            entry.advance_brokers(exhausted, 1),
            Err(StateFailure::Capacity)
        ));
        let mut exhausted = broker.clone();
        exhausted.bytes = 16 * 1024 * 1024;
        assert!(matches!(
            entry.advance_brokers(exhausted, 1),
            Err(StateFailure::Capacity)
        ));
        let foreign = registry
            .obtain(
                ContextBinding {
                    digest: [8; 32],
                    ..binding()
                },
                CONTEXT_TTL,
                "expiry".into(),
            )
            .unwrap();
        assert!(matches!(
            foreign.start_brokers(&"same-query", Some(&broker_token)),
            Err(StateFailure::InvalidCursor)
        ));
        drop(foreign);
        // Expiry is established without sleeping or weakening the product clock.
        let id = entry.id.clone();
        registry.entries.lock().unwrap().remove(&id);
        let unique = Arc::get_mut(&mut entry).unwrap();
        unique.created = unique.created.checked_sub(CONTEXT_TTL).unwrap();
        assert!(matches!(
            entry.start_brokers(&"same-query", Some(&broker_token)),
            Err(StateFailure::Expired)
        ));
    }

    #[test]
    fn worker_cursors_are_separate_from_brokers_resources_limits_contexts_and_budgets() {
        let registry = ContextRegistry::new();
        let entry = registry
            .obtain(binding(), CONTEXT_TTL, "expiry".into())
            .unwrap();
        let worker = entry.start_workers(&25, None).unwrap();
        let token = entry.seal_workers(&25, &worker).unwrap();
        assert!(entry.start_brokers(&25, Some(&token)).is_err());
        assert!(entry.start(&25, Some(&token)).is_err());
        assert!(entry.start_workers(&26, Some(&token)).is_err());
        let brokers = entry.start_brokers(&25, None).unwrap();
        let broker_token = entry.seal_brokers(&25, &brokers).unwrap();
        assert!(entry.start_workers(&25, Some(&broker_token)).is_err());
        let resource = entry.start(&25, None).unwrap();
        let resource_token = entry.seal(&25, &resource).unwrap();
        assert!(entry.start_workers(&25, Some(&resource_token)).is_err());
        let mut position = worker.clone();
        position.after_worker_id = Some("a".repeat(64));
        assert!(entry.seal_workers(&25, &position).is_err());
        let mut exhausted = worker.clone();
        exhausted.pages = 1000;
        assert!(matches!(
            entry.advance_workers(exhausted, 1),
            Err(StateFailure::Capacity)
        ));
        let mut exhausted = worker;
        exhausted.bytes = 16 * 1024 * 1024;
        assert!(matches!(
            entry.advance_workers(exhausted, 1),
            Err(StateFailure::Capacity)
        ));
        let foreign = registry
            .obtain(
                ContextBinding {
                    digest: [8; 32],
                    ..binding()
                },
                CONTEXT_TTL,
                "expiry".into(),
            )
            .unwrap();
        assert!(foreign.start_workers(&25, Some(&token)).is_err());
        for (index, cursor) in [token, broker_token, resource_token].iter().enumerate() {
            let wire = hex::decode(cursor).unwrap();
            assert_eq!(
                &wire[5..13],
                &u64::try_from(index).unwrap().to_be_bytes(),
                "one nonce sequence serves all cursor domains"
            );
        }
    }

    #[test]
    fn nil_worker_keyset_position_seals_for_a_following_page_without_widening_brokers() {
        let registry = ContextRegistry::new();
        let context = registry
            .obtain(binding(), CONTEXT_TTL, "expiry".into())
            .unwrap();
        let nil = uuid::Uuid::nil().to_string();
        let mut workers = context.start_workers(&25, None).unwrap();
        // A first human page whose last accepted legacy worker ID is nil still
        // needs to seal/advance when has_more is true.
        workers.after_worker_id = Some(nil.clone());
        let workers = context.advance_workers(workers, 256).unwrap();
        let next = context.seal_workers(&25, &workers).unwrap();
        assert_eq!(
            context
                .start_workers(&25, Some(&next))
                .unwrap()
                .after_worker_id
                .as_deref(),
            Some(nil.as_str())
        );
        let mut brokers = context.start_brokers(&25, None).unwrap();
        brokers.after_broker_id = Some(nil);
        assert!(
            matches!(
                context.seal_brokers(&25, &brokers),
                Err(StateFailure::InvalidCursor)
            ),
            "the independent broker UUID contract remains nonnil"
        );
    }
}
