# Security policy

## Reporting a vulnerability

Please report vulnerabilities **privately** through GitHub:
[Report a vulnerability](https://github.com/uptime-kinomi/uptime-kinomi/security/advisories/new)
(the "Security" tab of the repository). Do not open a public issue.

Please include the version (`uptime-kinomi version`), what an attacker
needs (network access, a configuration change, a malicious monitored
server, ...), and how to reproduce it. A configuration with secrets removed
helps.

kinomi is maintained by volunteers. The goal is to confirm a report within
a week, and to publish a fix and a GitHub security advisory as soon as the
fix is released. Reporters are credited in the advisory unless they prefer
not to be.

## Supported versions

Only the latest release gets security fixes. With the weekly update timer
(enabled by `install.sh` and the Ansible roles), installations get a fix
within a week of its release. When 2.0 comes out, the last 1.x release
keeps getting security fixes for at least six months, since the update
timer does not move to a new major version on its own.

## What counts

Anything in this repository: the `uptime-kinomi` program, `install.sh`,
the Ansible roles, the systemd units, the nginx and Apache examples, and
the release and signing process. Examples:

- a request to the built-in server that crashes kinomi, reads a file that
  is not a generated page, gets past Basic Auth, or counts as a push
  without the token;
- content from a monitored server or a push request that ends up in a log,
  page or notification (SPEC.md §11 says it never does);
- a way to make `self-update` or `install.sh` install something that is not
  a signed release.

What kinomi promises is in [SPEC.md §11](SPEC.md#11-security-summary) and
§9.4.

## Release keys

Releases are signed with one of two minisign keys (primary and backup,
[keys/](keys/)). Both are kept offline by the maintainer, never on GitHub
or in CI. `self-update` and `install.sh` only install a release whose
checksums are signed by one of them, so taking over the GitHub account or
CI alone is not enough to ship an update.

If a key is ever lost or exposed:

1. The next release is signed with the other key and contains a new
   key set without the exposed key. Installed versions accept it, because
   they trust both keys.
2. A GitHub security advisory and the changelog announce it, with the key
   ID. Releases signed with the exposed key after that date must not be
   installed.
3. Installations that update to that release no longer trust the exposed
   key.

An exposed key is only dangerous together with the ability to publish a
release on GitHub, so the window is short if the GitHub account stays safe.
