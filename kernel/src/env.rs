// kernel/src/env.rs — Environment variable storage
//
// Simple key-value store for environment variables.
// Used by SYS_SETENV/SYS_GETENV and shell.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// A single environment variable entry.
struct EnvVar {
    key: String,
    val: String,
}

/// Global environment store.
static ENV: Mutex<Vec<EnvVar>> = Mutex::new(Vec::new());

/// Initialize environment with defaults (called at boot).
pub fn init() {
    let mut env = ENV.lock();
    env.push(EnvVar {
        key: String::from("PATH"),
        val: String::from("/"),
    });
    env.push(EnvVar {
        key: String::from("USER"),
        val: String::from("root"),
    });
    env.push(EnvVar {
        key: String::from("HOME"),
        val: String::from("/"),
    });
    env.push(EnvVar {
        key: String::from("TERM"),
        val: String::from("xterm-256color"),
    });
}

/// Get an environment variable value.
pub fn get(key: &str) -> Option<String> {
    let env = ENV.lock();
    env.iter().find(|e| e.key == key).map(|e| e.val.clone())
}

/// Set an environment variable (create or update).
pub fn set(key: &str, val: &str) {
    let mut env = ENV.lock();
    for e in env.iter_mut() {
        if e.key == key {
            e.val = String::from(val);
            return;
        }
    }
    env.push(EnvVar {
        key: String::from(key),
        val: String::from(val),
    });
}

/// Unset an environment variable.
pub fn unset(key: &str) {
    let mut env = ENV.lock();
    env.retain(|e| e.key != key);
}

/// List all environment variables (for `env` command).
pub fn list_all() -> Vec<(String, String)> {
    let env = ENV.lock();
    env.iter().map(|e| (e.key.clone(), e.val.clone())).collect()
}
