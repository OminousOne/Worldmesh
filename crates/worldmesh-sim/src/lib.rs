//! Server Entity Groups: the game model and the processes that simulate entities. The only
//! place game code runs.

pub mod game;
pub mod group;

#[cfg(any(test, feature = "test-game"))]
pub mod test_game;
