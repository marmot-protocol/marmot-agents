//! The `pi` fuzz target: `marmot_agent_fuzz::pi`, whose documentation says what it checks.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| marmot_agent_fuzz::pi(data));
