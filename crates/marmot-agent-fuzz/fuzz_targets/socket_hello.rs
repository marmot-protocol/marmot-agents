//! The `socket_hello` fuzz target: `marmot_agent_fuzz::socket_hello`, whose documentation says what it checks.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| marmot_agent_fuzz::socket_hello(data));
