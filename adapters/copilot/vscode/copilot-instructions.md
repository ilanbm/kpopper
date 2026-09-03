<!-- kpopper:method -->
# kpopper — the method, condensed

Keep one file, PROVENANCE.yaml, at the project root, for work that will be revisited.
Record three things while working, never as a separate step:

1. Something taken from a source — record where, precisely enough to go back: the source
   AND the location within it. Say whether it is quoted, a paraphrase, or your reading.
2. Something worked out — record the rule, never the result.
3. Something concluded — record what it rests on, a `seen` snapshot of those values, and
   `wrong_if`: what would make it wrong. A judgment that cannot be wrong is an opinion.

Never restate recorded material from memory — reference it. Structure is added only when
something observable forces it. When a dependency moves: flag, never rewrite — re-running
is allowed, applying is not. A declared hole (`blocked_on`) is honest; prose in a
predicate field is not. A session's own prior knowledge is a source too — a `prior.*`
claim whose value is the confidence — and a judgment decided on one says in `reopened_by`
what would re-open it.

Open a session with `provenance.py open` instead of reading the record whole; ground a
subject with `pull`, trace what a change reaches with `affects`, gate on `check`.
<!-- /kpopper:method -->

At the start of a session, when `PROVENANCE.yaml` exists at the project root, run:

    python3 <plugin>/scripts/provenance.py open --chars 2000

and read that instead of the file whole. Before treating work as finished, run:

    python3 <plugin>/scripts/provenance.py check

and fix or declare (`blocked_on`) whatever it reports, rather than leaving the record
worse than you found it.
