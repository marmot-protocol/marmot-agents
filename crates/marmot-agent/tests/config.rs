//! The settings file: what it takes, and what it refuses.

use marmot_agent::config::{Config, ConfigError};
use marmot_agent::{FullActivity, GroupKey, PubKey, Who};

// NIP-19's own example.
const NPUB: &str = "npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg";
const HEX: &str = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";
const OTHER: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

fn settings(owner: &str, prompt: &str, invite: &str) -> String {
    format!(
        "home = \"/unused\"\nrelays = []\nowner = \"{owner}\"\nprompt = {prompt}\n\
         invite = [{invite}]\n[connector]\n"
    )
}

// A key in the settings is a public key in hex, in either case, or an npub; each is read as the
// lowercase hex MDK gives, so the owner and the people listed are recognised (S6).
#[test]
fn keys_are_read_in_any_form_a_person_has_them() {
    let upper = HEX.to_uppercase();
    for owner in [HEX, upper.as_str(), NPUB] {
        let text = settings(
            owner,
            &format!("{{ people = [\"{}\"] }}", OTHER.to_uppercase()),
            &format!("\"{NPUB}\""),
        );
        let config = Config::parse(&text).unwrap();
        assert_eq!(config.owner, PubKey::from(HEX), "{owner}");
        assert_eq!(config.prompt, Who::People(vec![PubKey::from(OTHER)]));
        assert_eq!(config.invite, vec![PubKey::from(HEX)]);
        let rights = config.rights();
        assert!(rights.allows(&PubKey::from(HEX), marmot_agent::Right::Approve));
    }
}

// A key the core cannot read is refused when the settings are read, naming the setting, instead
// of never matching anyone and locking the owner out (S6).
#[test]
fn a_key_that_is_not_one_is_refused() {
    let nsec = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";
    let short = &HEX[..62];
    let cases = [
        (settings("alice", "\"owner\"", ""), "owner"),
        (settings(nsec, "\"owner\"", ""), "owner"),
        (settings(short, "\"owner\"", ""), "owner"),
        (settings(HEX, "{ people = [\"bob\"] }", ""), "prompt"),
        (settings(HEX, "\"owner\"", "\"carol\""), "invite"),
    ];
    for (text, setting) in cases {
        match Config::parse(&text) {
            Err(ConfigError::Value(named)) => assert_eq!(named, setting),
            Err(other) => panic!("{setting}: {other}"),
            Ok(_) => panic!("{setting}: taken"),
        }
    }
    let approve = settings(HEX, "\"owner\"", "")
        .replace("prompt = \"owner\"", "approve = { people = [\"dave\"] }");
    assert!(matches!(
        Config::parse(&approve),
        Err(ConfigError::Value("approve"))
    ));
}

// Two cores on one home would write over each other's state: the second is refused when it
// opens the home, for as long as the first holds it.
#[test]
fn a_second_core_on_the_same_home_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let text = format!(
        "home = \"{}\"\nrelays = []\nowner = \"{HEX}\"\n[connector]\n",
        home.path().display()
    );
    let config = Config::parse(&text).unwrap();
    let first = marmot_agent::marmot::open(&config).expect("the first opens it");
    let second = marmot_agent::marmot::open(&config);
    assert!(second.is_err(), "a second core is refused");
    drop(first);
    assert!(
        marmot_agent::marmot::open(&config).is_ok(),
        "free again once the first has gone"
    );
}

// The folders a chat may move a coding harness to are whole paths, or start with `~`, the home;
// any other is refused when the settings are read, naming the setting (D-101).
#[test]
fn the_folders_a_chat_may_use_are_whole_paths() {
    let text = |folders: &str| {
        format!(
            "home = \"/unused\"\nrelays = []\nowner = \"{HEX}\"\nfolders = {folders}\n[connector]\n"
        )
    };
    let config = Config::parse(&text("[\"/srv/work\", \"~\", \"~/projects\"]")).unwrap();
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    assert_eq!(
        config.folders,
        Some(vec![
            "/srv/work".into(),
            home.clone(),
            home.join("projects")
        ])
    );
    assert!(matches!(
        Config::parse(&text("[\"projects\"]")),
        Err(ConfigError::Value("folders"))
    ));
    assert_eq!(
        Config::parse(&text("[]")).unwrap().folders,
        Some(Vec::new())
    );
}

// An ACP agent is given the driver's file tool, which listens on the loopback address, unless its
// settings switch it off (D-110).
#[test]
fn an_acp_agent_sends_files_only_when_its_settings_say_so() {
    let text = |extra: &str| {
        format!(
            "home = \"/unused\"\nrelays = []\nowner = \"{HEX}\"\n\
             [acp]\nname = \"opencode\"\ncommand = [\"opencode\", \"acp\"]\ncwd = \"/w\"\n{extra}"
        )
    };
    let acp = |extra: &str| Config::parse(&text(extra)).unwrap().acp.unwrap();
    assert!(acp("").send_files, "on by default");
    assert!(acp("send_files = true\n").send_files);
    assert!(!acp("send_files = false\n").send_files);
    assert!(matches!(
        Config::parse(&text("send_files = \"yes\"\n")),
        Err(ConfigError::Invalid)
    ));
}

// Which groups are sent the agent's full activity: none by default, those listed, or every chat
// (D-136). A group id is read in either case, as MDK gives it in lowercase hex.
#[test]
fn the_groups_sent_full_activity_are_listed_or_all() {
    let base = settings(HEX, "\"owner\"", "");
    let config = Config::parse(&base).unwrap();
    assert_eq!(config.full_activity, FullActivity::Groups(Vec::new()));

    let listed = base.replace("invite = []", "invite = []\nfull_activity = [\"ABCDEF01\"]");
    let config = Config::parse(&listed).unwrap();
    assert_eq!(
        config.full_activity,
        FullActivity::Groups(vec![GroupKey::from("abcdef01")])
    );

    let all = base.replace("invite = []", "invite = []\nfull_activity = \"all\"");
    assert_eq!(
        Config::parse(&all).unwrap().full_activity,
        FullActivity::All
    );

    for wrong in ["\"some\"", "[\"not hex\"]", "true"] {
        let text = base.replace(
            "invite = []",
            &format!("invite = []\nfull_activity = {wrong}"),
        );
        assert!(Config::parse(&text).is_err(), "{wrong}");
    }
}
