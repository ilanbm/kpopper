//! Where `add` files a new value whose prefix no section holds yet: where the values are. A
//! record that writes its values bare keeps them together, but a bare scalar counts as a value
//! only when no section holds one written out, and never in the questions or the sources.
use std::{fs, process::Command};

const CLAIMS: &str = "claims:\n  why_acme: {rests_on: [acme.seats], verdict: \"prefer Acme\", wrong_if: \"acme.seats < 50\", seen: {acme.seats: 120}}\n";

/// Adds `beta.price` to the record and returns the first line `add` prints, which names the
/// section the entry went into.
fn add(record: &str, value: &str) -> String {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("GROUNDING.yaml"), record).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(temp.path())
        .args(["add", "beta.price", value, "--as-of", "2026-09-22"])
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("KPOPPER_PRIVATE_HOME", temp.path().join("private"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn a_value_joins_the_section_of_bare_values() {
    assert_eq!(
        add(&format!("facts:\n  acme.seats: 120\n{CLAIMS}"), "v=3"),
        "add beta.price into facts, after acme.seats"
    );
}

#[test]
fn bare_scalars_do_not_count_where_a_section_holds_written_values() {
    let record = format!(
        "notes:\n  n.call: \"call Dana\"\n  n.lease: \"check the lease\"\n  n.term: \"ask about the term\"\nfacts:\n  acme.seats: {{v: 120}}\n{CLAIMS}"
    );
    assert_eq!(
        add(&record, "v=3"),
        "add beta.price into facts, after acme.seats"
    );
}

#[test]
fn questions_and_sources_are_never_a_home_for_values() {
    for (section, id, text) in [
        ("open", "q.budget", "is the budget fixed?"),
        ("sources", "pricing", "https://example.test/pricing"),
    ] {
        let record = format!(
            "{section}:\n  {id}: \"{text}\"\nclaims:\n  why_acme: {{rests_on: [{id}], verdict: \"prefer Acme\", seen: {{{id}: \"{text}\"}}}}\n"
        );
        assert_eq!(
            add(&record, "v=3"),
            "add beta.price into known, its first entry",
            "{section}"
        );
    }
}

#[test]
fn a_bare_value_with_a_new_prefix_is_still_a_question() {
    assert_eq!(
        add(&format!("facts:\n  acme.seats: 120\n{CLAIMS}"), "3"),
        "add beta.price into open, its first entry"
    );
}

#[test]
fn a_tie_between_sections_of_bare_values_goes_to_the_first_by_name() {
    let record = "zeta:\n  z.a: 1\nalpha:\n  a.b: 2\nclaims:\n  why: {rests_on: [z.a], verdict: keep, wrong_if: \"z.a < 0\", seen: {z.a: 1}}\n";
    assert_eq!(add(record, "v=3"), "add beta.price into alpha, after a.b");
}
