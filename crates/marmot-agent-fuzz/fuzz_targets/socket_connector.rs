//! The `socket_connector` fuzz target: `marmot_agent_fuzz::socket_connector`, whose documentation says what it checks.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| marmot_agent_fuzz::socket_connector(data));
