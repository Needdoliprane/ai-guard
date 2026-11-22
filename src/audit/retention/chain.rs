//! The one verifier of a tenant hash chain whose closed prefix may be sparse.
//!
//! Rows arrive in ascending sequence order. Below the retention base, a row
//! must hash to itself, link to its predecessor whenever that predecessor is
//! retained, and close its segment with the recorded boundary hash. Each
//! segment must keep exactly `archived - reclaimed` rows, so a deletion that no
//! segment accounts for is detected. Above the base, the chain is contiguous
//! from the boundary, exactly as a store without retention is from zero.
use super::super::event_codec::digest;
use sha2::{Digest, Sha256};

/// Commitment to the rows a segment keeps live: each kept row hashes to
/// itself, so its sequence and hash bind its bytes. Without it, a kept row
/// whose neighbours were reclaimed links to nothing locally and could be
/// rewritten unnoticed.
pub(crate) struct RetainedDigest(Sha256);

impl RetainedDigest {
    pub(crate) fn new() -> Self {
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:audit-retention-retained:v1\0");
        Self(digest)
    }
    pub(crate) fn add(&mut self, seq: i64, hash: &str) {
        self.0.update(seq.to_be_bytes());
        self.0.update(hash.as_bytes());
    }
    pub(crate) fn finish(self) -> String {
        hex::encode(self.0.finalize())
    }
}

pub(crate) const ZERO_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Accounting of one applied retention segment, as the live ledger records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SegmentBound {
    pub(crate) index: i64,
    pub(crate) first_seq: i64,
    pub(crate) last_seq: i64,
    pub(crate) last_hash: String,
    pub(crate) archived_events: i64,
    pub(crate) reclaimed_events: i64,
    pub(crate) retained_sha256: String,
}

impl SegmentBound {
    pub(crate) fn retained_events(&self) -> i64 {
        self.archived_events - self.reclaimed_events
    }
}

/// Segments must tile `1..=base` in order, each reclaiming at least one event
/// and keeping at least its boundary.
pub(crate) fn valid_segments(segments: &[SegmentBound]) -> bool {
    let mut next_first = 1_i64;
    for (position, segment) in segments.iter().enumerate() {
        let Some(expected_index) = i64::try_from(position).ok().and_then(|p| p.checked_add(1))
        else {
            return false;
        };
        let archived = segment
            .last_seq
            .checked_sub(segment.first_seq)
            .and_then(|span| span.checked_add(1));
        if segment.index != expected_index
            || segment.first_seq != next_first
            || archived != Some(segment.archived_events)
            || segment.reclaimed_events < 1
            || segment.reclaimed_events >= segment.archived_events
            || segment.last_hash.len() != 64
            || segment.retained_sha256.len() != 64
        {
            return false;
        }
        let Some(after) = segment.last_seq.checked_add(1) else {
            return false;
        };
        next_first = after;
    }
    true
}

pub(crate) struct RetainedChain {
    segments: Vec<SegmentBound>,
    digests: Vec<Option<RetainedDigest>>,
    retained: Vec<i64>,
    boundary_seen: Vec<bool>,
    cursor: usize,
    base: (i64, String),
    previous: Option<(i64, String)>,
    stored: u64,
}

impl RetainedChain {
    /// `None` when the segments do not tile a prefix.
    pub(crate) fn new(segments: Vec<SegmentBound>) -> Option<Self> {
        if !valid_segments(&segments) {
            return None;
        }
        let base = segments
            .last()
            .map(|segment| (segment.last_seq, segment.last_hash.clone()))
            .unwrap_or((0, ZERO_HASH.to_owned()));
        Some(Self {
            digests: segments
                .iter()
                .map(|_| Some(RetainedDigest::new()))
                .collect(),
            retained: vec![0; segments.len()],
            boundary_seen: vec![false; segments.len()],
            segments,
            cursor: 0,
            base,
            previous: None,
            stored: 0,
        })
    }

    pub(crate) fn base(&self) -> &(i64, String) {
        &self.base
    }

    /// Last accepted row, or the zero origin before any row.
    pub(crate) fn head(&self) -> (i64, String) {
        self.previous
            .clone()
            .unwrap_or_else(|| (0, ZERO_HASH.to_owned()))
    }

    pub(crate) fn stored(&self) -> u64 {
        self.stored
    }

    /// Accept one row; `false` is an integrity violation at `seq`.
    pub(crate) fn accept(
        &mut self,
        seq: i64,
        received: &str,
        prev_hash: &str,
        hash: &str,
        payload: &str,
    ) -> bool {
        if seq < 1
            || self.previous.as_ref().is_some_and(|(last, _)| *last >= seq)
            || hash != digest(seq, received, prev_hash, payload)
        {
            return false;
        }
        let linked = match &self.previous {
            Some((last, last_hash)) if last.checked_add(1) == Some(seq) => prev_hash == last_hash,
            None if seq == 1 => prev_hash == ZERO_HASH,
            // A gap is only possible inside the reclaimed prefix.
            _ => seq <= self.base.0,
        };
        if !linked {
            return false;
        }
        if seq <= self.base.0 {
            while self
                .segments
                .get(self.cursor)
                .is_some_and(|segment| segment.last_seq < seq)
            {
                self.cursor += 1;
            }
            let Some(segment) = self.segments.get(self.cursor) else {
                return false;
            };
            if seq == segment.last_seq {
                if hash != segment.last_hash {
                    return false;
                }
                self.boundary_seen[self.cursor] = true;
            }
            self.retained[self.cursor] += 1;
            if let Some(digest) = self.digests[self.cursor].as_mut() {
                digest.add(seq, hash);
            }
        }
        self.previous = Some((seq, hash.to_owned()));
        self.stored += 1;
        true
    }

    /// Every segment keeps exactly its retained rows, its boundary and the
    /// rows its digest commits to. Consumes the per-segment digests.
    pub(crate) fn complete(&mut self) -> bool {
        let mut complete = true;
        for (index, segment) in self.segments.iter().enumerate() {
            let digest = self.digests[index]
                .take()
                .map(RetainedDigest::finish)
                .unwrap_or_default();
            complete &= self.boundary_seen[index]
                && self.retained[index] == segment.retained_events()
                && digest == segment.retained_sha256;
        }
        complete
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(seq: i64, prev: &str, payload: &str) -> (i64, String, String, String, String) {
        let received = format!("2026-11-0{}T00:00:00.000Z", 1 + seq % 9);
        let hash = digest(seq, &received, prev, payload);
        (seq, received, prev.to_owned(), hash, payload.to_owned())
    }

    fn chain(length: i64) -> Vec<(i64, String, String, String, String)> {
        let mut rows = Vec::new();
        let mut prev = ZERO_HASH.to_owned();
        for seq in 1..=length {
            let next = row(seq, &prev, &format!("{{\"n\":{seq}}}"));
            prev = next.3.clone();
            rows.push(next);
        }
        rows
    }

    fn accept_all(
        chain: &mut RetainedChain,
        rows: &[(i64, String, String, String, String)],
    ) -> bool {
        rows.iter().all(|(seq, received, prev, hash, payload)| {
            chain.accept(*seq, received, prev, hash, payload)
        })
    }

    #[test]
    fn a_store_without_segments_is_contiguous_from_zero() {
        let rows = chain(5);
        let mut verifier = RetainedChain::new(vec![]).unwrap();
        assert!(accept_all(&mut verifier, &rows));
        assert!(verifier.complete());
        assert_eq!(verifier.head(), (5, rows[4].3.clone()));
        let mut gap = RetainedChain::new(vec![]).unwrap();
        assert!(gap.accept(1, &rows[0].1, &rows[0].2, &rows[0].3, &rows[0].4));
        assert!(!gap.accept(3, &rows[2].1, &rows[2].2, &rows[2].3, &rows[2].4));
    }

    #[test]
    fn a_sparse_prefix_verifies_only_with_exact_accounting_and_boundary() {
        let rows = chain(8);
        let mut kept_digest = RetainedDigest::new();
        kept_digest.add(2, &rows[1].3);
        kept_digest.add(5, &rows[4].3);
        let segment = SegmentBound {
            index: 1,
            first_seq: 1,
            last_seq: 5,
            last_hash: rows[4].3.clone(),
            archived_events: 5,
            reclaimed_events: 3,
            retained_sha256: kept_digest.finish(),
        };
        // Keep 2 and the boundary 5; reclaim 1, 3 and 4.
        let kept: Vec<_> = rows
            .iter()
            .filter(|row| ![1, 3, 4].contains(&row.0))
            .cloned()
            .collect();
        let mut verifier = RetainedChain::new(vec![segment.clone()]).unwrap();
        assert!(accept_all(&mut verifier, &kept));
        assert!(verifier.complete());
        assert_eq!(verifier.stored(), 5);

        // One more deletion that the segment does not account for.
        let unaccounted: Vec<_> = kept.iter().filter(|row| row.0 != 2).cloned().collect();
        let mut verifier = RetainedChain::new(vec![segment.clone()]).unwrap();
        assert!(accept_all(&mut verifier, &unaccounted));
        assert!(!verifier.complete());

        // A kept row rewritten consistently with itself still breaks the digest.
        let mut forged = kept.clone();
        let payload = "{\"n\":\"forged\"}".to_owned();
        forged[0].4 = payload.clone();
        forged[0].3 = digest(2, &forged[0].1, &forged[0].2, &payload);
        let mut verifier = RetainedChain::new(vec![segment.clone()]).unwrap();
        assert!(accept_all(&mut verifier, &forged));
        assert!(!verifier.complete());

        // The boundary itself removed: the suffix no longer links.
        let boundary: Vec<_> = kept.iter().filter(|row| row.0 != 5).cloned().collect();
        let mut verifier = RetainedChain::new(vec![segment.clone()]).unwrap();
        assert!(!accept_all(&mut verifier, &boundary));

        // A suffix gap is never accepted.
        let suffix: Vec<_> = kept.iter().filter(|row| row.0 != 7).cloned().collect();
        let mut verifier = RetainedChain::new(vec![segment]).unwrap();
        assert!(!accept_all(&mut verifier, &suffix));
    }

    #[test]
    fn segments_must_tile_the_prefix() {
        let hash = "a".repeat(64);
        let bound = |index, first_seq, last_seq, reclaimed_events| SegmentBound {
            index,
            first_seq,
            last_seq,
            last_hash: hash.clone(),
            archived_events: last_seq - first_seq + 1,
            reclaimed_events,
            retained_sha256: hash.clone(),
        };
        assert!(valid_segments(&[bound(1, 1, 5, 2), bound(2, 6, 9, 1)]));
        assert!(!valid_segments(&[bound(1, 2, 5, 2)]));
        assert!(!valid_segments(&[bound(1, 1, 5, 2), bound(2, 7, 9, 1)]));
        assert!(!valid_segments(&[bound(1, 1, 5, 0)]));
        assert!(!valid_segments(&[bound(1, 1, 5, 5)]));
        assert!(!valid_segments(&[bound(2, 1, 5, 2)]));
    }
}
