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
