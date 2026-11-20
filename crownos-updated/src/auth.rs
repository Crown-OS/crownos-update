//! Administrators are root and the members of `wheel`, read from the
//! passwd and group files. CrownOS keeps its accounts in those files, so the
//! daemon needs no NSS binding, and a fixture root can name its own
//! administrators. The two small synchronous reads run in the dispatch
//! closure on purpose: only the four verbs need them.

use std::{fs, path::Path};

use crate::paths::Paths;

const ROOT_UID: u32 = 0;
const ADMINISTRATOR_GROUP: &str = "wheel";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Administrator,
    ReadOnly,
}

pub fn access_of(uid: u32, paths: &Paths) -> Access {
    if uid == ROOT_UID {
        return Access::Administrator;
    }
    let read = |path: &Path| fs::read_to_string(path).unwrap_or_default();
    if is_administrator(uid, &read(&paths.passwd), &read(&paths.group)) {
        Access::Administrator
    } else {
        Access::ReadOnly
    }
}

fn is_administrator(uid: u32, passwd: &str, group: &str) -> bool {
    let Some((name, primary_gid)) = account(uid, passwd) else {
        return false;
    };
    group
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let group_name = fields.next()?;
            let gid = fields.nth(1)?.parse::<u32>().ok()?;
            let members = fields.next().unwrap_or_default();
            (group_name == ADMINISTRATOR_GROUP).then_some((gid, members))
        })
        .any(|(gid, members)| {
            gid == primary_gid || members.split(',').any(|member| member.trim() == name)
        })
}

fn account(uid: u32, passwd: &str) -> Option<(&str, u32)> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let account_uid = fields.nth(1)?.parse::<u32>().ok()?;
        let gid = fields.next()?.parse::<u32>().ok()?;
        (account_uid == uid).then_some((name, gid))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0::/root:/bin/bash\n\
        alice:x:1000:1000::/home/alice:/bin/bash\n\
        bob:x:1001:1001::/home/bob:/bin/bash\n\
        carol:x:1002:998::/home/carol:/bin/bash\n";
    const GROUP: &str = "root:x:0:root\nwheel:x:998:alice,dave\nalice:x:1000:\nbob:x:1001:\n";

    #[test]
    fn wheel_members_and_its_primary_members_are_administrators() {
        assert!(is_administrator(1000, PASSWD, GROUP));
        assert!(is_administrator(1002, PASSWD, GROUP));
        assert!(!is_administrator(1001, PASSWD, GROUP));
    }

    #[test]
    fn unknown_accounts_and_broken_files_are_not_administrators() {
        assert!(!is_administrator(4242, PASSWD, GROUP));
        assert!(!is_administrator(1000, "", GROUP));
        assert!(!is_administrator(1000, PASSWD, "wheel:x:nine:alice\n"));
    }

    #[test]
    fn a_member_named_like_a_prefix_is_not_a_member() {
        let group = "wheel:x:998:alice2\n";
        assert!(!is_administrator(1000, PASSWD, group));
    }
}
