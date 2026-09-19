# Read the research record from the CLI

These responses were captured from the six-paper [GROUNDING.yaml](GROUNDING.yaml).
The commands read the record without modifying it. Run them from the repository
root using an installed `kpop` command; from a source checkout, replace `kpop` with
`python3 scripts/kpopper`.

| What you want | Command | What it returns |
|---|---|---|
| A starting overview | `open` | Record contents, standing judgments, attention and open questions. |
| Check the recorded state | `check` | Consistency problems, changed premises and declared conditions. |
| Inspect a subject | `pull` | The reading or judgment, its reasons and relevant dependencies. |
| Trace the reach of a change | `affects` | Which recorded judgments depend on the selected subject. |

`ground` is the agent skill that guides these reads; it is not a `kpop ground`
subcommand. A new conversation can use the same CLI calls directly.

## Check the record

```sh
kpop check examples/dark-matter/GROUNDING.yaml
```

```text
NOTE cluster.mass_location: no predicate at all - decided; reopened by: The lensing reconstruction, gas-mass interpretation, line-of-sight structure or the gravit
NOTE cosmology.cold_component: no predicate at all - decided; reopened by: The likelihood, data combination or cosmological model changes enough to alter the inferre
NOTE evidence.shared_catalog: no predicate at all - decided; reopened by: A different catalog, sample selection or independently calibrated replication changes the 
NOTE galaxy.baryon_coupling: no predicate at all - decided; reopened by: Revised distances, inclinations, stellar mass-to-light conversion or independent samples m
NOTE galaxy.extended_mass: no predicate at all - decided; reopened by: The velocity readings, geometry, dynamical assumptions or an alternative explanation of th
NOTE particle.search_scope: no predicate at all - decided; reopened by: A later exposure, revised calibration, different interaction model or positive signal chan
NOTE synthesis.dark_matter: no predicate at all - decided; reopened by: A source finding or shared assumption is revised, or a worked alternative jointly addresse

7 judgments, 58 entries, 0 problems, 7 declared
```

Exit status: `0`. The seven scientific judgments have prose `reopened_by`
conditions, so the response lists them as declared review conditions. This is a
record-consistency result; evaluating the scientific judgments requires reading
and interpretation.

## Open the current context

```sh
kpop open examples/dark-matter/GROUNDING.yaml --chars 1000
```

```text
Six-paper worked example; selected source readings and an authored synthesis, not a complete review or live research-agent evaluation.
holds: rar (8) · research (8) · cmb (7) · lz (6) · paper (6) · sparc (6) · lensing (5) · rotation (5)
58 entries, 7 judgments, 5 open questions, updated 2026-09-19

nothing needs a person right now.

  ? research.alternatives: {'name': 'Compare explanations on the same evidence', 'question': 'Which  ...
  ? research.baryon_coupling: {'name': 'Explain the acceleration relation', 'question': 'Which mecha ...
  ? research.identity: {'name': 'What is the dark component?', 'question': 'Which physical candidate ...
  ? research.later_searches: {'name': 'Extend beyond the first LZ result', 'question': 'What do late ...
  ? research.shared_systematics: {'name': 'Track shared data and assumptions', 'question': 'How do S ...

standing:
  = cluster.mass_location: Under the adopted lensing interpretation, most gravit ...
  ... 6 more - raise --chars

next: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check

Mapping: complete
```

The five open questions remain in the record even when no changed premise is
currently flagged. `--chars` limits the overview. Workspace mapping notices can
differ by host or checkout; they describe workspace state alongside the record.

## Inspect a calculation

```sh
kpop pull cmb.dark_to_baryon_density examples/dark-matter/GROUNDING.yaml --budget 1100
```

```text
cmb.dark_to_baryon_density: 75/14 = cmb.omega_c_h2 / cmb.omega_b_h2 (Ratio of the abstract central physical de ...
+ cosmology.cold_component: Within base Lambda-CDM, the fitted physical cold-dark-matter density exceeds the b ...
    holds
    because: The density ratio connects two central values from the same fit. The model assumptions and
             retained lensing-amplitude tension limit the interpretation.
    reopened by: The likelihood, data combination or cosmological model changes enough to alter the inferred c ...

affects <entry> shows what a change reaches
```

The exact `75/14` result follows from the two rounded Planck abstract values.
`pull` also exposes the cosmological judgment that uses it. The ratio is not an
independent observation or an uncertainty estimate.

## Recover the argument

```sh
kpop pull synthesis.dark_matter examples/dark-matter/GROUNDING.yaml --budget 1700
```

```text
research.framework: Standard gravitational mass inference; base Lambda-CDM for the CMB result. (Adopted synthe ...
research.selection: Six source papers, selected findings and an authored synthesis. Not a complete review, a s ...
+ cluster.mass_location: Under the adopted lensing interpretation, most gravitating mass in this merger does n ...
    holds
    because: The spatial comparison is a distinct constraint from a rotation curve, while the paper's
             stronger proof language remains the authors' interpretation.
    reopened by: The lensing reconstruction, gas-mass interpretation, line-of-sight structure or the gravitati ...
+ cosmology.cold_component: Within base Lambda-CDM, the fitted physical cold-dark-matter density exceeds the b ...
    holds
    because: The density ratio connects two central values from the same fit. The model assumptions and
             retained lensing-amplitude tension limit the interpretation.
    reopened by: The likelihood, data combination or cosmological model changes enough to alter the inferred c ...
+ galaxy.extended_mass: Under the adopted gravitational interpretation, the rotation readings support extended ...
    holds
    because: The source records measured rotation separately from the authors' mass inference; the latter
             depends on the dynamical interpretation.
    reopened by: The velocity readings, geometry, dynamical assumptions or an alternative explanation of the s ...
+ synthesis.dark_matter: Under the stated models, the selected evidence supports a dark-matter account across  ...
    holds
    because: The chain keeps observations, modeling choices, evidence reuse and intermediate judgments
             visible. It neither adds paper votes nor calculates a probability that dark matter exists. The
             physical identity and a worked comparison of alternatives remain open.
    reopened by: A source finding or shared assumption is revised, or a worked alternative jointly addresses t ...

affects <entry> shows what a change reaches
```

The CLI's character budget can abbreviate long fields. The complete record and
[source guide](README.md#the-six-sources) retain all source locations, readings,
judgments and open questions.

## See what a premise reaches

```sh
kpop affects research.framework examples/dark-matter/GROUNDING.yaml
```

```text
cluster.mass_location
    via research.framework -> flagged only
cosmology.cold_component
    via research.framework -> flagged only
galaxy.extended_mass
    via research.framework -> flagged only
synthesis.dark_matter
    via research.framework -> flagged only

4 judgments reached
```

`affects` reads the dependency graph; it does not change the framework or refute
any conclusion. The [runnable replay](README.md#run-the-shared-record-example)
shows the separate act of proposing a changed framework and the resulting `MOVED`
notices.

<details>
<summary>Capture details</summary>

- Captured at: `2026-09-19T09:48:11.513426+00:00`.
- Reader: kpopper 1.7.0 source based on main `8115b44`; checked-in legacy record.
- Input SHA-256: `8b6a03d01392daa3f14a0aa6c1d4f3c30e9d55279cb8be99d288d08ef8d06c73`.
- All five commands exited `0`; the output above is retained verbatim, including
  abbreviations introduced by the CLI's budgets.

</details>
