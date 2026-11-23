//! The world: what a game is made of, assembled from plugins.
//!
//! ```ignore
//! let mut world = World::new(config);
//! world.plugin(Movement).plugin(Combat).plugin(Bots)
//!      .kind("hero", Bundle::new().with(Health(100)))
//!      .join(|player| Spawn::new("hero", 32.5, 32.5));
//! worldmesh::run(world).await
//! ```
//!
//! A [`Plugin`] registers what one feature needs: components, messages and their handlers,
//! systems, kinds. Every process builds the same world from the same code, so ids given in
//! registration order agree everywhere. Once built, the world is frozen into [`Rules`], which
//! the roles share.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use worldmesh_core::codec;
use worldmesh_core::config::WorldConfig;
use worldmesh_core::protocol::{EntityId, Part, Schema};

use crate::component::{decode_component, AnyComponent, Bundle, Component, Message, Wire};
use crate::entity::{Entity, Spawn};
use crate::me::{Incoming, Me};

/// A feature of a game: registers what it needs on the [`World`].
pub trait Plugin {
    fn build(&self, world: &mut World);
}

impl<F: Fn(&mut World)> Plugin for F {
    fn build(&self, world: &mut World) {
        self(world)
    }
}

/// When a system runs within a tick. Every entity runs `First` before any runs `Update`, and
/// so on; within a stage, systems run in the order they were registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    /// Decide: AI, reading what's around.
    First,
    /// Act: movement and most game rules.
    Update,
    /// Tidy up: respawns, asking "what's near me?" from where the entity ended up.
    Last,
}

pub(crate) type System = Arc<dyn Fn(&mut Me<'_>) + Send + Sync>;
pub(crate) type CommandHandler = Arc<dyn Fn(&mut Me<'_>, &dyn Any) + Send + Sync>;
pub(crate) type ActionHandler = Arc<dyn Fn(&mut Me<'_>, &dyn Any, &Incoming) + Send + Sync>;
type JoinFn = Arc<dyn Fn(EntityId) -> Spawn + Send + Sync>;
type Decode = fn(&[u8]) -> Option<Box<dyn Any + Send + Sync>>;
type FromJson = fn(Value) -> Result<Vec<u8>, String>;

/// What a message is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A player's request for their own entity.
    Command,
    /// Something an entity does to others.
    Action,
    /// Something players see happen.
    Event,
}

pub struct ComponentInfo {
    pub name: &'static str,
    pub visible: bool,
    pub(crate) type_id: TypeId,
    decode: fn(&[u8]) -> Option<Box<dyn AnyComponent>>,
}

pub struct MessageInfo {
    pub name: &'static str,
    pub category: Category,
    pub(crate) type_id: TypeId,
    pub(crate) decode: Decode,
    from_json: FromJson,
    pub(crate) commands: Vec<CommandHandler>,
    pub(crate) actions: Vec<ActionHandler>,
}

pub struct KindInfo {
    pub name: String,
    pub(crate) defaults: Bundle,
}

struct SystemInfo {
    stage: Stage,
    only: Option<TypeId>,
    run: System,
}

fn decode_any<T: Wire>(bytes: &[u8]) -> Option<Box<dyn Any + Send + Sync>> {
    codec::from_bytes::<T>(bytes).map(|m| Box::new(m) as Box<dyn Any + Send + Sync>)
}

/// A value from a client, checked against its type and encoded for the bus.
fn from_json<T: Wire>(value: Value) -> Result<Vec<u8>, String> {
    let typed: T = serde_json::from_value(value).map_err(|e| e.to_string())?;
    codec::to_bytes(&typed).ok_or_else(|| "couldn't encode".into())
}

/// A game, being assembled. See the [module docs](self).
pub struct World {
    config: WorldConfig,
    components: Vec<ComponentInfo>,
    messages: Vec<MessageInfo>,
    kinds: Vec<KindInfo>,
    systems: Vec<SystemInfo>,
    input: Option<(TypeId, FromJson)>,
    join: Option<JoinFn>,
    welcome: serde_json::Map<String, Value>,
}

impl World {
    pub fn new(config: WorldConfig) -> Self {
        Self {
            config,
            components: Vec::new(),
            messages: Vec::new(),
            kinds: Vec::new(),
            systems: Vec::new(),
            input: None,
            join: None,
            welcome: serde_json::Map::new(),
        }
    }

    pub fn config(&self) -> WorldConfig {
        self.config
    }

    /// Add a feature.
    pub fn plugin(&mut self, plugin: impl Plugin) -> &mut Self {
        plugin.build(self);
        self
    }

    fn register_component<T: Component>(&mut self, visible: bool) -> &mut Self {
        let type_id = TypeId::of::<T>();
        if let Some(c) = self.components.iter_mut().find(|c| c.type_id == type_id) {
            c.visible |= visible;
            return self;
        }
        assert!(
            !self.components.iter().any(|c| c.name == T::NAME),
            "two components are named {:?}: give one another name with #[worldmesh(name = \"...\")]",
            T::NAME
        );
        self.components.push(ComponentInfo {
            name: T::NAME,
            visible,
            type_id,
            decode: decode_component::<T>,
        });
        self
    }

    /// A component kept with the entity: it travels between groups, but nobody else sees it.
    pub fn component<T: Component>(&mut self) -> &mut Self {
        self.register_component::<T>(false)
    }

    /// A component that's also part of what others see: players who can see the entity, and
    /// other entities asking "what's near me?".
    pub fn visible<T: Component>(&mut self) -> &mut Self {
        self.register_component::<T>(true)
    }

    /// What players send as input: what they hold from now on (a direction, buttons), timed
    /// to a sub-step. One type per world; systems read it with [`Me::inputs`].
    pub fn input<I: Wire>(&mut self) -> &mut Self {
        let type_id = TypeId::of::<I>();
        if let Some((existing, _)) = self.input {
            assert!(
                existing == type_id,
                "a world has one input type; it's already registered"
            );
        }
        self.input = Some((type_id, from_json::<I>));
        self
    }

    fn message<M: Message>(&mut self, category: Category) -> &mut MessageInfo {
        let type_id = TypeId::of::<M>();
        if let Some(i) = self.messages.iter().position(|m| m.type_id == type_id) {
            let m = &mut self.messages[i];
            assert!(
                m.category == category,
                "{:?} is already registered as a {:?}, not a {:?}",
                M::NAME,
                m.category,
                category
            );
            return m;
        }
        assert!(
            !self.messages.iter().any(|m| m.name == M::NAME),
            "two messages are named {:?}: give one another name with #[worldmesh(name = \"...\")]",
            M::NAME
        );
        self.messages.push(MessageInfo {
            name: M::NAME,
            category,
            type_id,
            decode: decode_any::<M>,
            from_json: from_json::<M>,
            commands: Vec::new(),
            actions: Vec::new(),
        });
        self.messages.last_mut().expect("just pushed")
    }

    /// A command players can send their own entity, and what it does. Register more than one
    /// handler to have several features react; they run in registration order.
    pub fn command<C: Message>(
        &mut self,
        handler: impl Fn(&mut Me<'_>, &C) + Send + Sync + 'static,
    ) -> &mut Self {
        let run: CommandHandler = Arc::new(move |me, any| {
            if let Some(c) = any.downcast_ref::<C>() {
                handler(me, c)
            }
        });
        self.message::<C>(Category::Command).commands.push(run);
        self
    }

    /// An action entities send each other with [`Me::send`], and what it does to the entity
    /// it reaches. Several handlers run in registration order (a plugin can react to another
    /// plugin's actions).
    pub fn action<A: Message>(
        &mut self,
        handler: impl Fn(&mut Me<'_>, &A, &Incoming) + Send + Sync + 'static,
    ) -> &mut Self {
        let run: ActionHandler = Arc::new(move |me, any, from| {
            if let Some(a) = any.downcast_ref::<A>() {
                handler(me, a, from)
            }
        });
        self.message::<A>(Category::Action).actions.push(run);
        self
    }

    /// An event entities show to players with [`Me::emit`].
    pub fn event<E: Message>(&mut self) -> &mut Self {
        self.message::<E>(Category::Event);
        self
    }

    /// Run `f` for every entity, every tick, at `stage`.
    pub fn system(
        &mut self,
        stage: Stage,
        f: impl Fn(&mut Me<'_>) + Send + Sync + 'static,
    ) -> &mut Self {
        self.systems.push(SystemInfo {
            stage,
            only: None,
            run: Arc::new(f),
        });
        self
    }

    /// Run `f` every tick at `stage`, for entities that have a `T`.
    pub fn system_for<T: Component>(
        &mut self,
        stage: Stage,
        f: impl Fn(&mut Me<'_>) + Send + Sync + 'static,
    ) -> &mut Self {
        self.systems.push(SystemInfo {
            stage,
            only: Some(TypeId::of::<T>()),
            run: Arc::new(f),
        });
        self
    }

    /// A kind of entity, and the components it starts with. Spawn one with
    /// [`Spawn::new`]`(name, x, y)`. Its components are registered if they aren't yet (as
    /// not visible).
    ///
    /// Calling it again with the same name adds to that kind, so each plugin can give a
    /// shared kind its own components (a combat plugin adds health to "hero" and "bot", a
    /// bots plugin adds a brain to "bot").
    pub fn kind(&mut self, name: &str, defaults: Bundle) -> &mut Self {
        for c in &defaults.items {
            let type_id = c.as_any().type_id();
            if !self.components.iter().any(|i| i.type_id == type_id) {
                assert!(
                    !self.components.iter().any(|i| i.name == c.type_name()),
                    "two components are named {:?}: give one another name with #[worldmesh(name = \"...\")]",
                    c.type_name()
                );
                self.components.push(ComponentInfo {
                    name: c.type_name(),
                    visible: false,
                    type_id,
                    decode: c.decoder(),
                });
            }
        }
        match self.kinds.iter_mut().find(|k| k.name == name) {
            Some(kind) => kind.defaults.merge(defaults),
            None => self.kinds.push(KindInfo {
                name: name.to_string(),
                defaults,
            }),
        }
        self
    }

    /// Where a player appears when they connect, and as what.
    pub fn join(&mut self, f: impl Fn(EntityId) -> Spawn + Send + Sync + 'static) -> &mut Self {
        self.join = Some(Arc::new(f));
        self
    }

    /// Something every player gets when they connect (the map, say), under `key`.
    pub fn welcome(&mut self, key: &str, value: impl Serialize) -> &mut Self {
        let value = serde_json::to_value(value).expect("the welcome value converts to JSON");
        self.welcome.insert(key.to_string(), value);
        self
    }

    /// Check everything fits together and freeze it.
    pub fn build(self) -> Arc<Rules> {
        let World {
            config,
            mut components,
            messages,
            kinds,
            systems,
            input,
            join,
            welcome,
        } = self;
        let join = join
            .expect("the world needs a join function: world.join(|player| Spawn::new(kind, x, y))");
        components.shrink_to_fit();
        let component_ids = components
            .iter()
            .enumerate()
            .map(|(i, c)| (c.type_id, i as u16))
            .collect();
        let message_ids = messages
            .iter()
            .enumerate()
            .map(|(i, m)| (m.type_id, i as u16))
            .collect();
        let message_names = messages
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name, i as u16))
            .collect();
        let kind_ids = kinds
            .iter()
            .enumerate()
            .map(|(i, k)| (k.name.clone(), i as u16))
            .collect();
        let mut staged: [Vec<(Option<TypeId>, System)>; 3] = Default::default();
        for s in systems {
            staged[s.stage as usize].push((s.only, s.run));
        }
        Arc::new(Rules {
            config,
            components,
            component_ids,
            messages,
            message_ids,
            message_names,
            kinds,
            kind_ids,
            systems: staged,
            input: input.map(|(_, f)| f),
            join,
            welcome: Value::Object(welcome),
        })
    }
}

/// A built world: everything registered, with ids. Shared by every role.
pub struct Rules {
    pub config: WorldConfig,
    components: Vec<ComponentInfo>,
    component_ids: HashMap<TypeId, u16>,
    messages: Vec<MessageInfo>,
    message_ids: HashMap<TypeId, u16>,
    message_names: HashMap<&'static str, u16>,
    kinds: Vec<KindInfo>,
    kind_ids: HashMap<String, u16>,
    pub(crate) systems: [Vec<(Option<TypeId>, System)>; 3],
    input: Option<FromJson>,
    join: JoinFn,
    welcome: Value,
}

impl Rules {
    /// The names behind every id, for clients.
    pub fn schema(&self) -> Schema {
        Schema {
            components: self.components.iter().map(|c| c.name.to_string()).collect(),
            messages: self.messages.iter().map(|m| m.name.to_string()).collect(),
            kinds: self.kinds.iter().map(|k| k.name.clone()).collect(),
        }
    }

    pub fn welcome(&self) -> &Value {
        &self.welcome
    }

    pub fn component(&self, id: u16) -> Option<&ComponentInfo> {
        self.components.get(usize::from(id))
    }

    pub fn component_id<T: Component>(&self) -> Option<u16> {
        self.component_ids.get(&TypeId::of::<T>()).copied()
    }

    pub(crate) fn component_id_of(&self, type_id: TypeId) -> Option<u16> {
        self.component_ids.get(&type_id).copied()
    }

    pub fn message(&self, id: u16) -> Option<&MessageInfo> {
        self.messages.get(usize::from(id))
    }

    pub fn message_id<M: Message>(&self) -> Option<u16> {
        self.message_ids.get(&TypeId::of::<M>()).copied()
    }

    pub fn kind_id(&self, name: &str) -> Option<u16> {
        self.kind_ids.get(name).copied()
    }

    pub fn kind_name(&self, id: u16) -> &str {
        self.kinds.get(usize::from(id)).map_or("?", |k| &k.name)
    }

    /// Decode a component (by id) from the bus.
    pub fn decode_component(&self, id: u16, bytes: &[u8]) -> Option<Box<dyn AnyComponent>> {
        (self.component(id)?.decode)(bytes)
    }

    /// A player's input from a client, checked against the world's input type.
    pub fn input_from_json(&self, value: Value) -> Result<Vec<u8>, String> {
        let f = self.input.ok_or("this world takes no input")?;
        f(value)
    }

    /// A player's command from a client: (message id, encoded), if `name` is a command and
    /// `value` reads as it.
    pub fn command_from_json(&self, name: &str, value: Value) -> Result<(u16, Vec<u8>), String> {
        let id = *self
            .message_names
            .get(name)
            .ok_or_else(|| format!("no message is named {name:?}"))?;
        let m = &self.messages[usize::from(id)];
        if m.category != Category::Command {
            return Err(format!("{name:?} isn't a command"));
        }
        Ok((id, (m.from_json)(value)?))
    }

    /// Where and as what a player appears.
    pub fn join(&self, player: EntityId) -> Spawn {
        (self.join)(player)
    }

    /// A spawn, encoded for the bus: (kind id, extra components). None (and an error logged)
    /// if the kind or a component isn't registered.
    pub fn encode_spawn(&self, spawn: &Spawn) -> Option<(u16, Vec<Part>)> {
        let Some(kind) = self.kind_id(&spawn.kind) else {
            tracing::error!(kind = %spawn.kind, "spawning an unknown kind: register it with world.kind()");
            return None;
        };
        let parts = self.encode(spawn.with.items.iter(), false)?;
        Some((kind, parts))
    }

    /// Components as parts, sorted by id; `visible_only` keeps just the visible ones. None
    /// if one isn't registered.
    pub(crate) fn encode<'a>(
        &self,
        components: impl Iterator<Item = &'a Box<dyn AnyComponent>>,
        visible_only: bool,
    ) -> Option<Vec<Part>> {
        let mut parts = Vec::new();
        for c in components {
            let Some(id) = self.component_id_of(c.as_any().type_id()) else {
                tracing::error!(
                    component = c.type_name(),
                    "a component that isn't registered: add world.component::<..>()"
                );
                return None;
            };
            if visible_only && !self.components[usize::from(id)].visible {
                continue;
            }
            parts.push((id, bytes::Bytes::from(c.encode()?)));
        }
        parts.sort_by_key(|(id, _)| *id);
        Some(parts)
    }

    /// A new entity: its kind's components, then the extras on top.
    pub fn new_entity(
        &self,
        id: EntityId,
        x: f32,
        y: f32,
        player: bool,
        kind: u16,
        extras: &[Part],
    ) -> Option<Entity> {
        let info = self.kinds.get(usize::from(kind))?;
        let mut entity = Entity::new(id, x, y, player, kind);
        for c in &info.defaults.items {
            entity.put(c.clone_box());
        }
        for (cid, bytes) in extras {
            entity.put(self.decode_component(*cid, bytes)?);
        }
        Some(entity)
    }
}
