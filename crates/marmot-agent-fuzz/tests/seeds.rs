//! Every fuzz target's check over every one of its seeds, on the stable compiler: the checks
//! build, and what the repository holds passes them. The fuzzing itself is nightly's
//! (`.github/workflows/nightly.yml`).

use marmot_agent_fuzz::seeds::{TARGETS, of};

#[test]
fn every_check_passes_its_seeds() {
    for (target, check) in TARGETS {
        let seeds = of(target);
        assert!(!seeds.is_empty(), "{target} has seeds");
        for seed in &seeds {
            check(seed);
        }
    }
}

/// Each check takes what is not its input at all.
#[test]
fn every_check_passes_over_nothing_and_noise() {
    for (_, check) in TARGETS {
        for noise in [
            &b""[..],
            b"\n",
            b"\xff\xfe",
            b"{",
            b"[[\"p\"]]",
            b"1{}\n0[]\n",
        ] {
            check(noise);
        }
    }
}

/// Regression for #242: a command or method name ending in `_chunk]` produces a single-message
/// `response to ...` / `error response to ...` native payload, not an ACP streamed chunk list.
#[test]
fn translator_fuzz_checks_handle_chunk_suffix_in_command_or_method() {
    marmot_agent_fuzz::pi(
        br#"1{"id":"4","type":"response","command":"get__chunk]","message":"/mcp"}
"#,
    );
    marmot_agent_fuzz::codex(
        br#"0{"id":1,"method":"foo_chunk]","params":{}}
1{"id":1,"error":{"code":-32601,"message":"not found"}}
"#,
    );
    marmot_agent_fuzz::acp(
        br#"0{"jsonrpc":"2.0","id":1,"method":"foo_chunk]","params":{}}
1{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"not found"}}
"#,
    );
}

/// cargo-fuzz takes every program of the package for a fuzz target, and the nightly job fuzzes
/// each one it lists, so the package's programs are the targets and nothing else: one that is
/// not, such as what writes the seeds, would be run as a fuzzer and pass without fuzzing.
#[test]
fn the_package_has_a_program_for_each_target_and_no_other() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(folder.join("Cargo.toml")).expect("the manifest");
    let mut programs = Vec::new();
    let mut in_bin = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
        } else if in_bin && let Some(name) = line.strip_prefix("name = ") {
            programs.push(name.trim_matches('"').to_string());
        }
    }
    let mut targets: Vec<String> = TARGETS.iter().map(|(t, _)| t.to_string()).collect();
    programs.sort();
    targets.sort();
    assert_eq!(programs, targets);
    // Cargo would find these by itself.
    assert!(!folder.join("src/main.rs").exists());
    assert!(!folder.join("src/bin").exists());
}
