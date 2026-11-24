//! The `acp` fuzz target: `marmot_agent_fuzz::acp`, whose documentation says what it checks.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| marmot_agent_fuzz::acp(data));
