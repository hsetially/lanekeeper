//! From an OS Login name to an application user (T7, Q32, A18).
//!
//! The sentinel names the person behind a local write by the **OS Login user** auditd kept for the session (`ada_example_com`: Google's
//! login name for `ada@example.com`). An attribution is more useful with the application user in it, so the registry maps the name:
//!
//! 1. **The admin's map first** (`os_login_user_map`, one row for each login name an admin wrote): an exact match on the name, to the user
//!    the admin chose. The admin said so, so the user's status is not asked: the record is history, and a user who has since been disabled
//!    still made the change.
//! 2. **Then an unambiguous match on email.** The OS Login form of each **active** user's email (lower case, every character that is not
//!    an ASCII letter or digit written as `_`) is compared with the OS Login form of the name; the user is the answer only when **exactly
//!    one** active user has that form. Two users with one form (`a.b@x.com` and `a_b@x.com`) are ambiguous, and an ambiguous name is
//!    **not guessed at**: the attribution names the OS Login user and no application user (`Actor::OsLogin`).
//! 3. **Otherwise** the name stands alone, as `Actor::OsLogin`.
//!
//! The reading is direct SQL on `users.email` because the `Users` port has no lookup by email (A18). The form is computed in SQL with
//! `translate` and `regexp_replace`, which are locale independent, and in Rust by [`os_login_form`]; a test holds the two to the same
//! answer on awkward input. The admin operations that **write** the map are not built here (A7).

use std::collections::{BTreeSet, HashMap};

use domain::{Guid, UserId};
use sqlx::PgConnection;

use crate::db::Db;
use crate::error::RegistryError;

/// The admin's map: the users of some login names (`$1` the names), exact match, served by the primary key.
pub const ADMIN_MAP_SQL: &str =
    "SELECT os_login, user_tid, user_oid FROM os_login_user_map WHERE os_login = ANY($1::text[])";

/// The users whose email has one of the forms in `$1`, when **exactly one** active user has that form (the rest are ambiguous and are not
/// returned). `translate` lowers the case without a locale: after `regexp_replace` only ASCII letters, digits and `_` are left.
pub const EMAIL_SQL: &str = "SELECT f.form, (array_agg(f.tid))[1], (array_agg(f.oid))[1] FROM ( \
         SELECT translate(regexp_replace(email, '[^a-zA-Z0-9]', '_', 'g'), \
                          'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz') AS form, tid, oid \
         FROM users WHERE status = 'active') f \
     WHERE f.form = ANY($1::text[]) GROUP BY f.form HAVING count(*) = 1";

/// The OS Login form of a name or an email: ASCII letters in lower case, digits kept, everything else (including every character that
/// is not ASCII) written as `_`.
pub(crate) fn os_login_form(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn user_of(tid: &str, oid: &str) -> Option<UserId> {
    Some(UserId::new(Guid::parse(tid).ok()?, Guid::parse(oid).ok()?))
}

/// The application user of each of the OS Login names that has one. A name that is not in the answer is unmapped (the attribution names
/// the OS Login user). Two statements at most, whatever the number of names.
pub(crate) async fn resolve(
    db: &Db,
    conn: &mut PgConnection,
    logins: &BTreeSet<String>,
) -> Result<HashMap<String, UserId>, RegistryError> {
    let mut found: HashMap<String, UserId> = HashMap::new();
    if logins.is_empty() {
        return Ok(found);
    }
    let names: Vec<String> = logins.iter().cloned().collect();
    let admin: Vec<(String, String, String)> = db
        .run(
            "os_login_map",
            sqlx::query_as(ADMIN_MAP_SQL).bind(&names).fetch_all(&mut *conn),
        )
        .await?;
    for (login, tid, oid) in admin {
        if let Some(user) = user_of(&tid, &oid) {
            found.insert(login, user);
        }
    }
    // The names the admin did not map, by the form an email would give them.
    let rest: Vec<&String> = logins.iter().filter(|l| !found.contains_key(*l)).collect();
    if rest.is_empty() {
        return Ok(found);
    }
    let mut by_form: HashMap<String, Vec<&String>> = HashMap::new();
    for login in rest {
        by_form.entry(os_login_form(login)).or_default().push(login);
    }
    let forms: Vec<String> = by_form.keys().cloned().collect();
    let rows: Vec<(String, String, String)> = db
        .run(
            "os_login_email",
            sqlx::query_as(EMAIL_SQL).bind(&forms).fetch_all(&mut *conn),
        )
        .await?;
    for (form, tid, oid) in rows {
        let (Some(user), Some(names)) = (user_of(&tid, &oid), by_form.get(&form)) else {
            continue;
        };
        for login in names {
            found.insert((*login).clone(), user.clone());
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_email_and_its_os_login_name_have_one_form() {
        assert_eq!(os_login_form("ada@example.com"), "ada_example_com");
        assert_eq!(os_login_form("ada_example_com"), "ada_example_com");
        assert_eq!(
            os_login_form("Ada.Lovelace@Example.COM"),
            "ada_lovelace_example_com"
        );
        assert_eq!(os_login_form("a-b+c@my-corp.com"), "a_b_c_my_corp_com");
    }

    #[test]
    fn what_is_not_ascii_is_written_as_an_underscore() {
        assert_eq!(os_login_form("é@x.io"), "__x_io");
        assert_eq!(os_login_form("İ@x.io"), "__x_io", "no locale decides the case");
        assert_eq!(os_login_form(""), "");
    }

    #[test]
    fn the_form_has_the_length_of_the_text_in_characters() {
        let text = "ab.c@d-é.com";
        assert_eq!(os_login_form(text).chars().count(), text.chars().count());
    }

    #[test]
    fn two_emails_with_one_form_are_the_ambiguity_the_sql_refuses_to_guess() {
        assert_eq!(os_login_form("a.b@x.com"), os_login_form("a_b@x.com"));
        assert!(EMAIL_SQL.contains("HAVING count(*) = 1"));
        assert!(EMAIL_SQL.contains("status = 'active'"));
    }

    #[test]
    fn the_sql_form_is_the_rust_form() {
        // The two have to agree, or a user would be matched by one and missed by the other: the class and the case map are the same text.
        assert!(EMAIL_SQL.contains("'[^a-zA-Z0-9]', '_', 'g'"));
        assert!(EMAIL_SQL.contains("'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz'"));
    }
}
