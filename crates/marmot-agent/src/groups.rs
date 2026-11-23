//! What `marmot-agent groups` prints: each group the agent is in, for its owner to find the ids
//! that `full_activity` takes (D-136). A group's id is printed to the terminal only, never logged
//! (D-071).

use crate::keys::GroupKey;

/// A group the agent is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// Its id, in hex, as MDK gives it and `full_activity` takes it.
    pub group: GroupKey,
    /// Its name; empty for a direct chat.
    pub name: String,
    /// Whether it is the owner's two-person chat with the agent, which is sent the agent's full
    /// activity whatever the settings say.
    pub owners_chat: bool,
    /// Whether `full_activity` lists it, or is `"all"`.
    pub full_activity: bool,
}

/// The table the command prints: a heading, then one line for each group.
pub fn table(groups: &[Listed]) -> String {
    if groups.is_empty() {
        return "The agent is in no groups.\n".into();
    }
    let heading = ["group id", "owner's chat", "in full_activity", "name"];
    let yes = |b: bool| if b { "yes" } else { "no" };
    let rows: Vec<[String; 4]> = groups
        .iter()
        .map(|g| {
            let name: String = g
                .name
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let name = if name.trim().is_empty() {
                "(no name)".into()
            } else {
                name
            };
            [
                g.group.as_str().to_string(),
                yes(g.owners_chat).into(),
                yes(g.full_activity).into(),
                name,
            ]
        })
        .collect();
    let width = |i: usize| {
        let longest = rows.iter().map(|r| r[i].chars().count()).max();
        longest.unwrap_or(0).max(heading[i].len())
    };
    let widths = [width(0), width(1), width(2)];
    let line = |cells: [&str; 4]| {
        format!(
            "{:w0$}  {:w1$}  {:w2$}  {}\n",
            cells[0],
            cells[1],
            cells[2],
            cells[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
        )
    };
    let mut shown = line(heading);
    for row in &rows {
        shown += &line([&row[0], &row[1], &row[2], &row[3]]);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(group: &str, name: &str, owners_chat: bool, full_activity: bool) -> Listed {
        Listed {
            group: GroupKey(group.into()),
            name: name.into(),
            owners_chat,
            full_activity,
        }
    }

    // One line for each group, under a heading, the name last and on the same line, whatever it
    // holds.
    #[test]
    fn each_group_is_one_line() {
        let shown = table(&[
            listed("6c01", "", true, false),
            listed("6c0203", "Team\nand\tfriends", false, true),
        ]);
        assert_eq!(
            shown,
            "group id  owner's chat  in full_activity  name\n\
             6c01      yes           no                (no name)\n\
             6c0203    no            yes               Team and friends\n"
        );
    }

    #[test]
    fn no_groups_is_said() {
        assert_eq!(table(&[]), "The agent is in no groups.\n");
    }
}
