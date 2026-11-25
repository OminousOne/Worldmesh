//! Sure delivery, like TCP over a bus that can drop messages.
//!
//! The bus (core NATS) delivers each message at most once, and drops messages when a process
//! falls behind. An envelope sent for sure delivery carries [`Envelope::ack`]: whoever gets it
//! confirms it (in a batch, once per tick), and its sender keeps it in [`Unconfirmed`] and sends
//! it again until that confirmation comes back:
//!
//! - a confirmation normally comes back within two ticks: the receiver applies the envelope on
//!   its next tick and confirms it at the end of that tick;
//! - anything unconfirmed after [`RESEND_AFTER_MS`] is sent again, then after twice as long
//!   (at most [`MAX_GAP_MS`]), and so on, until [`GIVE_UP_MS`], when it's counted as lost;
//! - receivers remember what they applied ([`Handled`]) for [`REMEMBER_MS`], longer than
//!   anyone keeps sending, and refuse (but still confirm) anything sent longer ago than that,
//!   so a copy never applies twice.
//!
//! Envelopes sent without `ack` are sent once and never confirmed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::protocol::{EntityId, Envelope};
use crate::subjects::{entity_subject, group_inbox_subject, is_spatial_id, spatial_inbox_subject};
use crate::time;

/// Send again if no confirmation came back within this long (doubling each time).
pub const RESEND_AFTER_MS: u64 = 400;
/// The longest wait between two copies.
pub const MAX_GAP_MS: u64 = 1_000;
/// Stop sending and count it as lost after this long.
pub const GIVE_UP_MS: u64 = 5_000;
/// Receivers remember what they applied for this long: longer than anyone keeps sending.
pub const REMEMBER_MS: u64 = 10_000;

/// [`RESEND_AFTER_MS`] in ticks (at least 3: a confirmation takes up to two).
pub fn resend_after_ticks() -> u64 {
    time::ticks_in(RESEND_AFTER_MS).max(3)
}

pub fn give_up_ticks() -> u64 {
    time::ticks_in(GIVE_UP_MS)
}

pub fn remember_ticks() -> u64 {
    time::ticks_in(REMEMBER_MS)
}

/// Sent at `sent_tick`, arriving at `now`: too long ago to tell whether a copy was already
/// applied. Such a copy is confirmed but not applied (its sender stopped sending long before).
pub fn too_old(sent_tick: u64, now: u64) -> bool {
    sent_tick + remember_ticks() < now
}

/// One process that must confirm an envelope, and where a copy goes if it doesn't.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The group or Spatial Layer Cell that must confirm it. None: an entity, wherever it is
    /// (any group confirming it counts).
    pub by: Option<u32>,
    pub subject: String,
}

impl Target {
    /// An entity, wherever it is: copies go to its subject, which its group listens to.
    pub fn entity(id: EntityId) -> Self {
        Self {
            by: None,
            subject: entity_subject(id),
        }
    }

    /// One group or Spatial Layer Cell, through its inbox.
    pub fn inbox(id: u32) -> Self {
        let subject = if is_spatial_id(id) {
            spatial_inbox_subject(id)
        } else {
            group_inbox_subject(id)
        };
        Self {
            by: Some(id),
            subject,
        }
    }
}

struct Waiting {
    env: Envelope,
    targets: Vec<Target>,
    /// The tick to send it again on, the wait after that, and the tick to give up on.
    next: u64,
    gap: u64,
    give_up: u64,
}

/// What to send again this tick, and what was given up on.
#[derive(Debug, Default)]
pub struct Due {
    pub resend: Vec<(String, Envelope)>,
    pub lost: Vec<Envelope>,
}

/// Envelopes one process sent for sure delivery and hasn't heard back about, by
/// [`Envelope::key`].
#[derive(Default)]
pub struct Unconfirmed {
    waiting: BTreeMap<(EntityId, u64), Waiting>,
    /// Copies sent again, and envelopes given up on, since the last [`Unconfirmed::take_counts`].
    resent: u64,
    lost: u64,
}

impl Unconfirmed {
    /// Keep `env` (just sent, at tick `now`) until every target confirms it.
    pub fn track(&mut self, env: &Envelope, targets: Vec<Target>, now: u64) {
        if targets.is_empty() {
            return;
        }
        let gap = resend_after_ticks();
        self.waiting.insert(
            env.key(),
            Waiting {
                env: env.clone(),
                targets,
                next: now + gap,
                gap,
                give_up: now + give_up_ticks(),
            },
        );
    }

    /// `by` confirmed the envelope with this key.
    pub fn confirm(&mut self, by: u32, key: (EntityId, u64)) {
        let Some(w) = self.waiting.get_mut(&key) else {
            return;
        };
        w.targets.retain(|t| t.by.is_some_and(|b| b != by));
        if w.targets.is_empty() {
            self.waiting.remove(&key);
        }
    }

    /// Copies to send at tick `now`, each to whoever hasn't confirmed yet, and what's been
    /// unconfirmed for too long (forgotten, and counted as lost).
    pub fn due(&mut self, now: u64) -> Due {
        let mut due = Due::default();
        let mut gone = Vec::new();
        let max_gap = time::ticks_in(MAX_GAP_MS).max(resend_after_ticks());
        for (key, w) in self.waiting.iter_mut() {
            if now >= w.give_up {
                gone.push(*key);
                continue;
            }
            if w.next > now {
                continue;
            }
            w.env.resends = w.env.resends.saturating_add(1);
            for t in &w.targets {
                due.resend.push((t.subject.clone(), w.env.clone()));
            }
            self.resent += w.targets.len() as u64;
            w.gap = (w.gap * 2).min(max_gap);
            w.next = now + w.gap;
        }
        for key in gone {
            if let Some(w) = self.waiting.remove(&key) {
                self.lost += 1;
                due.lost.push(w.env);
            }
        }
        due
    }

    /// Stop waiting for processes that are gone (`alive` is false for them). An envelope
    /// nobody is left to confirm is forgotten.
    pub fn retain_confirmers(&mut self, alive: impl Fn(u32) -> bool) {
        for w in self.waiting.values_mut() {
            w.targets.retain(|t| t.by.is_none_or(&alive));
        }
        self.waiting.retain(|_, w| !w.targets.is_empty());
    }

    /// Forget envelopes for which `keep` is false (a player who left, say).
    pub fn retain(&mut self, keep: impl Fn(&Envelope) -> bool) {
        self.waiting.retain(|_, w| keep(&w.env));
    }

    /// Still waiting for the envelope with this key.
    pub fn is_waiting(&self, key: (EntityId, u64)) -> bool {
        self.waiting.contains_key(&key)
    }

    /// Still waiting for anything `sender` sent.
    pub fn any_from(&self, sender: EntityId) -> bool {
        self.waiting
            .range((sender, 0)..=(sender, u64::MAX))
            .next()
            .is_some()
    }

    /// Envelopes still waiting for a confirmation.
    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// (copies sent again, envelopes given up on) since the last call.
    pub fn take_counts(&mut self) -> (u64, u64) {
        (
            std::mem::take(&mut self.resent),
            std::mem::take(&mut self.lost),
        )
    }
}

/// What was already applied, remembered for [`REMEMBER_MS`]: copies of a sure envelope can
/// arrive more than once (sent again, or forwarded after a handoff as well as sent directly).
#[derive(Debug, Clone, PartialEq)]
pub struct Handled<K: Ord> {
    /// (tick noted, key), oldest first.
    order: VecDeque<(u64, K)>,
    keys: BTreeSet<K>,
}

impl<K: Ord> Default for Handled<K> {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            keys: BTreeSet::new(),
        }
    }
}

impl<K: Ord + Clone> Handled<K> {
    /// Note `key` at tick `now`. False if it was already noted (within [`REMEMBER_MS`]).
    pub fn first(&mut self, key: K, now: u64) -> bool {
        let keep = remember_ticks();
        while let Some((t, _)) = self.order.front() {
            if t + keep >= now {
                break;
            }
            let (_, old) = self.order.pop_front().expect("checked");
            self.keys.remove(&old);
        }
        if !self.keys.insert(key.clone()) {
            return false;
        }
        self.order.push_back((now, key));
        true
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

// On the wire (an entity's handoff carries what it was sent): a list of (tick noted, key).
impl<K: Ord + Serialize> Serialize for Handled<K> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.order.serialize(s)
    }
}

impl<'de, K: Ord + Clone + Deserialize<'de>> Deserialize<'de> for Handled<K> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let order = VecDeque::<(u64, K)>::deserialize(d)?;
        let keys = order.iter().map(|(_, k)| k.clone()).collect();
        Ok(Self { order, keys })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Address, Facts, Kind};

    fn env(from: EntityId, seq: u64) -> Envelope {
        Envelope {
            to: Address::Entity { id: 9 },
            from: Facts {
                id: from,
                ..Facts::default()
            },
            via: 1,
            seq,
            trace: 0,
            hops: 0,
            kind: Kind::Action {
                msg: 0,
                body: Vec::new(),
            },
            ack: None,
            resends: 0,
        }
    }

    #[test]
    fn sent_again_with_growing_gaps_until_confirmed() {
        let mut u = Unconfirmed::default();
        u.track(&env(1, 1), vec![Target::entity(9)], 100);
        let gap = resend_after_ticks();
        assert!(u.due(100 + gap - 1).resend.is_empty());
        let first = u.due(100 + gap).resend;
        assert_eq!(first.len(), 1);
        assert_eq!((first[0].0.as_str(), first[0].1.resends), ("entity.9", 1));
        assert!(
            u.due(100 + gap * 2).resend.is_empty(),
            "waits twice as long"
        );
        assert_eq!(u.due(100 + gap * 3).resend.len(), 1);
        u.confirm(7, (1, 1)); // any group confirms an entity's envelope
        assert!(u.is_empty());
        assert!(u.due(100 + gap * 10).resend.is_empty());
        assert_eq!(u.take_counts(), (2, 0));
    }

    #[test]
    fn each_named_target_must_confirm_and_copies_go_only_to_the_silent_ones() {
        let mut u = Unconfirmed::default();
        u.track(
            &env(1, 1),
            vec![Target::inbox(0x8001), Target::inbox(0x8002)],
            0,
        );
        u.confirm(0x8001, (1, 1));
        assert!(u.is_waiting((1, 1)));
        let again = u.due(resend_after_ticks()).resend;
        assert_eq!(
            again.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(),
            vec![spatial_inbox_subject(0x8002)]
        );
        u.confirm(0x8002, (1, 1));
        assert!(!u.is_waiting((1, 1)));
    }

    #[test]
    fn given_up_after_a_while_and_counted_as_lost() {
        let mut u = Unconfirmed::default();
        u.track(&env(1, 1), vec![Target::entity(9)], 0);
        let mut lost = Vec::new();
        for t in 0..=give_up_ticks() {
            lost.extend(u.due(t).lost);
        }
        assert_eq!(lost.len(), 1);
        assert!(u.is_empty());
        assert_eq!(u.take_counts().1, 1);
    }

    #[test]
    fn gone_confirmers_are_not_waited_for() {
        let mut u = Unconfirmed::default();
        u.track(&env(1, 1), vec![Target::inbox(1), Target::inbox(2)], 0);
        u.retain_confirmers(|id| id == 1);
        u.confirm(1, (1, 1));
        assert!(u.is_empty());
    }

    #[test]
    fn handled_keys_are_remembered_for_a_while_then_forgotten() {
        let mut h = Handled::default();
        assert!(h.first((1, 1), 100));
        assert!(!h.first((1, 1), 100 + remember_ticks()), "still remembered");
        assert!(h.first((1, 2), 101));
        assert!(h.first((1, 1), 101 + remember_ticks()), "forgotten by now");
        let wire = crate::codec::to_bytes(&h).unwrap();
        let back: Handled<(u32, u64)> = crate::codec::from_bytes(&wire).unwrap();
        assert_eq!(back, h);
        assert!(too_old(0, remember_ticks() + 1));
        assert!(!too_old(0, remember_ticks()));
    }
}
