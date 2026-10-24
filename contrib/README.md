# contrib

| Path | What |
|---|---|
| `examples/config/` | A complete example configuration directory (all file types). Checked by CI. |
| `systemd/user/` | User service: runs as your user, no root needed. Plus a weekly self-update timer. |
| `systemd/system/` | Hardened system service with a dedicated `kinomi` user. |
| `nginx/`, `apache/` | Reverse proxy examples: mode A (kinomi serves everything) and mode B (the proxy serves the files and does the Basic Auth), see SPEC.md §9.1. |
| `ansible-role-kinomi/` | Ansible role: create the user, validate the configuration with `check-config`, deploy as a user service, enable lingering. |
| `release/` | Maintainer tools: signing a release. |
