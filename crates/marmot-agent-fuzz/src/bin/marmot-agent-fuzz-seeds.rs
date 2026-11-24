//! Writes each fuzz target's seeds into `<folder>/<target>/`, one file each, for cargo-fuzz to
//! start from: `marmot-agent-fuzz-seeds <folder>`.

fn main() {
    let folder = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("usage: marmot-agent-fuzz-seeds <folder>");
    for (target, _) in marmot_agent_fuzz::seeds::TARGETS {
        let into = folder.join(target);
        std::fs::create_dir_all(&into).expect("a folder for the seeds");
        let seeds = marmot_agent_fuzz::seeds::of(target);
        for (n, seed) in seeds.iter().enumerate() {
            std::fs::write(into.join(format!("seed-{n:04}")), seed).expect("a seed written");
        }
        println!("{target}: {} seeds", seeds.len());
    }
}
