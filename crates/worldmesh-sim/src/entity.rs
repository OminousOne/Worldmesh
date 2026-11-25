//! Entities: an id, a position, a kind, and components.

use std::any::TypeId;

use serde::{Deserialize, Serialize};
use worldmesh_core::protocol::{EntityId, Part};
use worldmesh_core::reliable::Handled;

use crate::component::{AnyComponent, Bundle, Component};
use crate::world::Rules;

/// An entity: what Worldmesh needs to know (id, position, whether a player controls it, its
/// kind) and its components.
#[derive(Clone)]
pub struct Entity {
    pub id: EntityId,
    /// Position in tiles. Where it stands decides who can see it.
    pub x: f32,
    pub y: f32,
    /// Controlled by a connected player.
    pub player: bool,
    pub(crate) kind: u16,
    pub(crate) components: Vec<Box<dyn AnyComponent>>,
}

impl Entity {
    pub(crate) fn new(id: EntityId, x: f32, y: f32, player: bool, kind: u16) -> Self {
        Self {
            id,
            x,
            y,
            player,
            kind,
            components: Vec::new(),
        }
    }

    /// The kind's id (its name: [`Rules::kind_name`], or `Me::kind`).
    pub fn kind_id(&self) -> u16 {
        self.kind
    }

    fn index<T: Component>(&self) -> Option<usize> {
        self.components
            .iter()
            .position(|c| c.as_any().type_id() == TypeId::of::<T>())
    }

    /// This entity's `T`, if it has one.
    pub fn get<T: Component>(&self) -> Option<&T> {
        self.index::<T>()
            .and_then(|i| self.components[i].as_any().downcast_ref())
    }

    /// This entity's `T` to change, if it has one.
    pub fn get_mut<T: Component>(&mut self) -> Option<&mut T> {
        let i = self.index::<T>()?;
        self.components[i].as_any_mut().downcast_mut()
    }

    pub fn has<T: Component>(&self) -> bool {
        self.index::<T>().is_some()
    }

    pub(crate) fn has_type(&self, type_id: TypeId) -> bool {
        self.components
            .iter()
            .any(|c| c.as_any().type_id() == type_id)
    }

    /// Add a component, replacing one of the same type.
    pub(crate) fn put(&mut self, component: Box<dyn AnyComponent>) {
        let type_id = component.as_any().type_id();
        self.components.retain(|c| c.as_any().type_id() != type_id);
        self.components.push(component);
    }

    pub(crate) fn take<T: Component>(&mut self) -> Option<T> {
        let i = self.index::<T>()?;
        let boxed = self.components.remove(i);
        boxed.as_any().downcast_ref::<T>().cloned()
    }

    /// The names of the components it has, for debugging.
    pub fn component_names(&self) -> Vec<&'static str> {
        self.components.iter().map(|c| c.type_name()).collect()
    }
}

impl std::fmt::Debug for Entity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entity")
            .field("id", &self.id)
            .field("x", &self.x)
            .field("y", &self.y)
            .field("player", &self.player)
            .field("kind", &self.kind)
            .field("components", &self.component_names())
            .finish()
    }
}

/// A new entity: a kind (registered with `World::kind`), where it appears, and any
/// components to add to (or replace in) the kind's.
#[derive(Clone, Debug)]
pub struct Spawn {
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub(crate) with: Bundle,
}

impl Spawn {
    pub fn new(kind: &str, x: f32, y: f32) -> Self {
        Self {
            kind: kind.to_string(),
            x,
            y,
            with: Bundle::new(),
        }
    }

    /// Start with this component (on top of the kind's).
    pub fn with<T: Component>(mut self, component: T) -> Self {
        self.with = self.with.with(component);
        self
    }
}

/// What Worldmesh keeps about an entity besides its components. Travels on handoffs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Meta {
    /// The tick it was last simulated on, so the next group never simulates a tick twice.
    pub last_tick: u64,
    /// Numbers the envelopes it sends, so receivers never apply one twice.
    pub seq: u64,
    /// (sender, number, sent by the gateway) of the envelopes delivered to it lately, so a
    /// copy (sent again, or forwarded as well as sent directly) never applies twice. A
    /// player's commands (numbered by the gateway) and their entity's actions (numbered by
    /// the entity) can share a number, hence the flag.
    pub recent: Handled<(EntityId, u64, bool)>,
    /// Where it stood at the end of recent ticks: (tick, x, y).
    pub trail: Vec<(u64, f32, f32)>,
    /// A player's inputs not simulated yet: (tick, sub-step, sequence number, input). Only the
    /// owning group hears a player's inputs, so they travel with the entity on a handoff.
    pub pending: Vec<(u64, u8, u64, serde_bytes::ByteBuf)>,
}

/// An entity and its bookkeeping, as simulated by a group.
#[derive(Clone)]
pub(crate) struct Record {
    pub entity: Entity,
    pub meta: Meta,
}

/// A record on the bus (a handoff).
#[derive(Serialize, Deserialize)]
struct WireRecord {
    id: EntityId,
    x: f32,
    y: f32,
    player: bool,
    kind: u16,
    parts: Vec<Part>,
    meta: Meta,
}

impl Record {
    pub(crate) fn encode(&self, rules: &Rules) -> Option<Vec<u8>> {
        let e = &self.entity;
        let parts = rules.encode(e.components.iter(), false)?;
        let wire = WireRecord {
            id: e.id,
            x: e.x,
            y: e.y,
            player: e.player,
            kind: e.kind,
            parts,
            meta: self.meta.clone(),
        };
        worldmesh_core::codec::to_bytes(&wire)
    }

    pub(crate) fn decode(rules: &Rules, bytes: &[u8]) -> Option<Self> {
        let w: WireRecord = worldmesh_core::codec::from_bytes(bytes)?;
        let mut entity = Entity::new(w.id, w.x, w.y, w.player, w.kind);
        for (id, bytes) in &w.parts {
            entity.put(rules.decode_component(*id, bytes)?);
        }
        Some(Record {
            entity,
            meta: w.meta,
        })
    }
}
