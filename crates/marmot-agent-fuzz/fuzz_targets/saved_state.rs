//! The `saved_state` fuzz target: `marmot_agent_fuzz::saved_state`, whose documentation says what it checks.

#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| marmot_agent_fuzz::saved_state(data));
