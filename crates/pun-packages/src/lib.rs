pub mod autoload;
pub mod bins;
pub mod classmap;
pub mod composer_config;
pub mod constraint;
pub mod error;
pub mod fetch;
pub mod fingerprint;
pub mod install;
pub mod installed;
pub mod lock;
pub mod manifest_edit;
pub mod paths;
pub mod php;
pub mod plugin_host;
pub mod plugins;
pub mod resolver;
pub mod scripts;
pub mod store;
pub mod verify;
pub mod version;

/// Looks up an environment variable. The binary passes `&|k| std::env::var(k).ok()`; tests pass fixed values.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;
