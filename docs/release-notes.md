# Writing release notes

Use the first `## Unreleased` section in `CHANGELOG.md` to explain changes that a
reader, plugin user or operator needs to understand. A feature PR updates these
notes alongside its documentation. It does not change version files.

Lead with the resulting behavior. Include defaults, opt-in or opt-out controls,
compatibility, migration steps if needed, and limitations that affect use. Group
related changes under short third-level headings such as `Added`, `Changed`,
`Fixed`, `Compatibility` or a feature name. Link detailed guides and examples.
Do not paste a commit log, private evaluation output or development transcripts.

For example:

```markdown
## Unreleased

### Changed

- Follow-up queries use eligible cited source IDs as ranking hints while still
  searching the whole record. Use `--no-auto-anchors` for an independent query.

### Experimental

- Incremental delivery remains off by default. A smaller packet does not establish
  a reduction in total task time or tokens.
```

Keep one Unreleased section above released versions. `## [Unreleased]` is also
accepted. HTML template comments are removed when the release notes are promoted.
Do not add release date or version placeholders inside the notes.

## What automation does

The release bot chooses the largest `Bump:` declared by PRs merged since the last
release. It moves the curated notes into `## VERSION — DATE`, adds `Included
changes` with PR links, and carries the same notes into the release PR body.
Existing historical sections stay unchanged. If there are no curated notes, the
previous title-based changelog remains available as a fallback.

After promotion, the next user-visible PR creates a fresh Unreleased section.
This is deliberate: the public version's entry must describe only that release.
The feature PR's `Bump:` controls version selection; prose headings do not.

Describe validation precisely. A test result applies to its tested source or
artifact. Publication, installation, a fresh host session and measured user value
are separate claims. An incomplete experiment is not a passing result. Release
notes should explain material limitations without publishing private receipts.

## Publication

The [CI guide](ci.md) is the publication contract. The release candidate is checked
and built first. The publish workflow releases those exact native artifacts and
publishes/verifies the crate through its existing trusted registry flow. After
publication the release gate is rerun before the version PR merges. Do not run a
separate preliminary `cargo publish` as part of this workflow.

The changelog generator is covered by `python3 -m unittest tests.test_release`,
including note promotion, historical preservation, duplicate/misplaced section
refusal and legacy behavior.
