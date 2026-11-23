//! Components and messages: the typed pieces a game is made of.
//!
//! A **component** is a piece of an entity's state (`Health`, `Inventory`, `Brain`). Entities
//! are an id, a position and a set of components; components travel with the entity between
//! groups, and the ones registered as visible make up what others see of it.
//!
//! A **message** is something sent: a player's command, an action between entities, or an
//! event shown to players. Each has a stable name, used on the wire and by clients.
//!
//! Derive both with `#[derive(Component)]` and `#[derive(Message)]` (from the `worldmesh`
//! crate); the name defaults to the type's name in snake_case.

use std::any::Any;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Anything that can travel between processes.
pub trait Wire: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {}
impl<T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> Wire for T {}

/// A piece of an entity's state. Register it with [`crate::World::component`] (kept to the
/// entity) or [`crate::World::visible`] (also part of what others see).
pub trait Component: Wire {
    /// The name clients and other processes know it by. Unique among components.
    const NAME: &'static str;
}

/// A command, action or event. Register it with [`crate::World::command`],
/// [`crate::World::action`] or [`crate::World::event`].
pub trait Message: Wire {
    /// The name clients and other processes know it by. Unique among messages.
    const NAME: &'static str;
}

/// A component behind a box, so an entity can hold components of any type.
pub trait AnyComponent: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn clone_box(&self) -> Box<dyn AnyComponent>;
    fn encode(&self) -> Option<Vec<u8>>;
    fn type_name(&self) -> &'static str;
    /// Reads this component's type back from the bus.
    fn decoder(&self) -> fn(&[u8]) -> Option<Box<dyn AnyComponent>>;
}

/// Read a `T` from the bus, boxed.
pub(crate) fn decode_component<T: Component>(bytes: &[u8]) -> Option<Box<dyn AnyComponent>> {
    worldmesh_core::codec::from_bytes::<T>(bytes).map(|c| Box::new(c) as Box<dyn AnyComponent>)
}

impl<T: Component> AnyComponent for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn clone_box(&self) -> Box<dyn AnyComponent> {
        Box::new(self.clone())
    }
    fn encode(&self) -> Option<Vec<u8>> {
        worldmesh_core::codec::to_bytes(self)
    }
    fn type_name(&self) -> &'static str {
        T::NAME
    }
    fn decoder(&self) -> fn(&[u8]) -> Option<Box<dyn AnyComponent>> {
        decode_component::<T>
    }
}

impl Clone for Box<dyn AnyComponent> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Components to start an entity with: a kind's defaults, or a spawn's extras.
#[derive(Clone, Default)]
pub struct Bundle {
    pub(crate) items: Vec<Box<dyn AnyComponent>>,
}

impl Bundle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a component (replacing one of the same type already in the bundle).
    pub fn with<T: Component>(mut self, component: T) -> Self {
        self.items.retain(|c| c.as_any().type_id() != std::any::TypeId::of::<T>());
        self.items.push(Box::new(component));
        self
    }
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.items.iter().map(|c| c.type_name())).finish()
    }
}
