//! The derive macros, used the way a game uses them.

use serde::{Deserialize, Serialize};
use worldmesh::{Bundle, Component, Message, Spawn, Stage, World, WorldConfig};

#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Health(i32);

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
#[worldmesh(name = "combat.cooldowns")]
struct Cooldowns {
    ready_at: Vec<u64>,
}

#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct UseAbility {
    slot: u8,
}

#[test]
fn names_default_to_snake_case_and_can_be_chosen() {
    assert_eq!(<Health as worldmesh::Component>::NAME, "health");
    assert_eq!(<Cooldowns as worldmesh::Component>::NAME, "combat.cooldowns");
    assert_eq!(<UseAbility as worldmesh::Message>::NAME, "use_ability");
}

#[test]
fn derived_types_register_and_show_up_in_the_schema() {
    let mut world = World::new(WorldConfig::default());
    world
        .visible::<Health>()
        .component::<Cooldowns>()
        .command(|me, c: &UseAbility| {
            if let Some(cd) = me.get_mut::<Cooldowns>() {
                cd.ready_at.push(u64::from(c.slot));
            }
        })
        .system(Stage::Update, |_| {})
        .kind("hero", Bundle::new().with(Health(100)).with(Cooldowns::default()))
        .join(|_| Spawn::new("hero", 1.0, 1.0));
    let schema = world.build().schema();
    assert_eq!(schema.components, vec!["health", "combat.cooldowns"]);
    assert_eq!(schema.messages, vec!["use_ability"]);
    assert_eq!(schema.kinds, vec!["hero"]);
}
