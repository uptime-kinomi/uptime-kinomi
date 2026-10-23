# contrib

| Path | What |
|---|---|
| `examples/config/` | A complete example configuration directory (all file types). Checked by CI. |
| `systemd/user/` | User service: runs as your user, no root needed. |
| `systemd/system/` | Hardened system service with a dedicated `kinomi` user. |
| `nginx/`, `apache/` | Reverse proxy examples: mode A (kinomi serves everything) and mode B (the proxy serves the files and does the Basic Auth), see SPEC.md §9.1. |
| `ansible/` | Example playbook: validate the configuration with `check-config`, then deploy. |
| `release/` | Maintainer tools: signing a release. |
