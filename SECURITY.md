# Security policy

## Report a vulnerability privately

**Before public release: the maintainer must add a private reporting address here.**

Please do not report vulnerabilities in public issues or pull requests, or attach
private project records, credentials or source documents. If GitHub shows a
**Report a vulnerability** button on this repository's [Security page](https://github.com/ilanbm/kpopper/security),
you can use that private channel.

A useful report includes:

- The affected kpopper version or commit and installation method.
- Your agent host, operating system and relevant runtime versions.
- The affected component and the access an attacker would need.
- Reproduction steps using invented data and the expected security boundary.
- The potential impact and a suggested fix, if you have one.

The maintainer will assess the report and coordinate a fix and disclosure with you.
Please allow time to investigate before publishing details. Response times depend
on maintainer availability; this project does not offer a security response SLA.

## Supported versions

Security fixes target the [latest stable release](https://github.com/ilanbm/kpopper/releases/latest).
Older releases and development checkouts do not have a separate security maintenance
commitment. If you found a vulnerability on an older version, report it even if you
cannot reproduce it on the latest release.

## Security boundaries

kpopper runs with the permissions of the local user or agent host. Review plugin hooks
and host permissions when installing or updating it.

- Measurement recipes in `.kpopper/measure.yaml` are executable code. Review them
  before using `remeasure --run`, including in a cloned repository. The plain
  `remeasure` command shows the plan without running recipes. See
  [measurement checks](CONTRIBUTING.md#record-and-measurement-checks).
- Records and exported pages or documents can contain source material and private
  project information. Review what an artifact includes before sharing it.
- The experimental `core/v1` profile executes the packaged native arithmetic runtime.
  Extraction is automatic and offline; it does not require a compiler or checked-session
  setup. Report unexpected executable selection, archive extraction or library-loading
  behavior privately. See the [runtime documentation](scripts/reasoning/native/README.md).
- A passing check evaluates the declared record and conditions. It does not verify
  source-world truth, establish authorization or replace a security review. The
  [checked-session limits](docs/checked-sessions.md) and
  [experimental core's assurance limits](docs/reasoning-core.md#runtime-licensing-and-assurance)
  describe the respective runtimes' scope.

Reports about unintended execution, access outside the intended project, unsafe
handling of document content or private-data disclosure are especially useful.
