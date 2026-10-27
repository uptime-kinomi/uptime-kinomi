# contrib

| Path | What |
|---|---|
| `install.sh` | One-line installation for the current user (no root), see the README and SPEC.md §10.5. |
| `examples/config/` | A complete example configuration directory (all file types). Checked by CI. |
| `systemd/user/` | User service: runs as your user, no root needed. Plus a weekly self-update timer. |
| `systemd/system/` | Hardened system service with a dedicated `kinomi` user. |
| `nginx/`, `apache/` | Reverse proxy examples: mode A (kinomi serves everything) and mode B (the proxy serves the files and does the Basic Auth), see SPEC.md §9.1. |
| `ansible-role-user/` | Ansible role, kinomi runs as the login user: validate the configuration with `check-config`, deploy as a user service, enable lingering. No root. |
| `ansible-role-root/` | Ansible role, logged in as root: packages, create `KINOMI_USER`, then the same as `ansible-role-user` for that user. |
| `release/` | Maintainer tools: signing a release. |
