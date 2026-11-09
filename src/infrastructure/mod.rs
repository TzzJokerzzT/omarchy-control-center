//! Infrastructure: everything that touches the outside world.

pub mod config;
pub mod history;
pub mod logging;
pub mod paths;
pub mod preferences;
pub mod process;
pub mod reclaim;
pub mod system;
#[cfg(test)]
pub mod test_support;
pub mod validate;
