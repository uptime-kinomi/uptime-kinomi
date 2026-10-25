# ansible-role-kinomi

Deploys uptime-kinomi as a systemd **user** service for `KINOMI_USER`
(SPEC.md §10.5, §10.6):

- installs `curl`, `wget` and `minisign` with apt (become);
- creates `KINOMI_USER` if it does not exist (become);
- enables lingering, so the service starts at boot without a login;
- if `~/.local/bin/uptime-kinomi` does not exist: downloads the latest
  release for the CPU, verifies the minisign signature of `SHA256SUMS` and
  the checksum, and installs it. An existing binary is never touched;
  updates are done by `self-update`;
- copies the configuration to a staging directory, runs `check-config` on
  it, and only then replaces `~/.config/uptime-kinomi`;
- installs `contrib/systemd/user/uptime-kinomi.service` and (re)starts it;
- installs and enables the weekly self-update timer
  (`uptime-kinomi-update.timer`), unless `kinomi_auto_update: false`.

The role uses `contrib/systemd/user/` from this repository, so keep it inside
`contrib/` (or copy that directory next to it).

## Variables

| Variable | Default | Meaning |
|---|---|---|
| `KINOMI_USER` | `kinomi` | User kinomi runs as. |
| `kinomi_auto_update` | `true` | Enable the weekly self-update timer. With `false`, update with `uptime-kinomi self-update`. |
| `kinomi_config` | `{{ playbook_dir }}/files/config/` | Configuration directory (start from `contrib/examples/config`). |

## Become

The apt task always needs become. If `KINOMI_USER` is the Ansible login user,
everything else runs without it. For any other user, the user is created,
files are copied and lingering is enabled with become. Run with `-K`
(`--ask-become-pass`) if sudo needs a password.

## Example

`site.yml` next to `files/`:

```yaml
- name: Deploy uptime-kinomi
  hosts: monitoring
  roles:
    - role: ansible-role-kinomi
      vars:
        KINOMI_USER: kinomi
```

```bash
ansible-playbook -i pi1.example.org, -K site.yml
```

with `roles_path` (or `ANSIBLE_ROLES_PATH`) pointing at `contrib/`.
For a system-wide service, use `contrib/systemd/system/` instead.
