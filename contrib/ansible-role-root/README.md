# ansible-role-root

Deploys uptime-kinomi with Ansible logged in as **root**, for a systemd
**user** service of `KINOMI_USER` (SPEC.md §10.5, §10.6). kinomi itself never
runs as root. Without root access, use
[`ansible-role-user`](../ansible-role-user/) instead.

- installs `curl`, `wget`, `minisign` and `sudo` with apt;
- creates `KINOMI_USER` if it does not exist;
- enables lingering for it, so the service starts at boot without a login;
- if `~KINOMI_USER/.local/bin/uptime-kinomi` does not exist: downloads the
  latest release for the CPU, verifies the minisign signature of
  `SHA256SUMS` and the checksum, and installs it. An existing binary is
  never touched; updates are done by `self-update`;
- sends the configuration as one archive (Ansible Vault files decrypted,
  symlinks followed), unpacks it into an emptied staging directory, runs
  `check-config` there as `KINOMI_USER`, and only then replaces
  `~/.config/uptime-kinomi` with it, if anything differs. Files removed or
  renamed in your configuration disappear on the target too. The role stops
  if `data_dir`, `log_dir` or `output_dir` is inside the configuration
  directory (they would be deleted). The target needs `tar`, `gzip` and
  `diff`;
- installs the units from `contrib/systemd/user/`, (re)starts the service
  and enables the weekly self-update timer, unless `kinomi_auto_update: false`.

All files belong to `KINOMI_USER`; `check-config` and `systemctl --user` run
as `KINOMI_USER` (via `sudo`, pipelined, so no ACL tools are needed).

The role uses `contrib/systemd/user/` from this repository, so keep it inside
`contrib/` (or copy that directory next to it).

## Variables

| Variable | Default | Meaning |
|---|---|---|
| `KINOMI_USER` | `kinomi` | User kinomi runs as; created if it does not exist. |
| `kinomi_config` | `{{ playbook_dir }}/files/config/` | Configuration directory (start from `contrib/examples/config`). |
| `kinomi_auto_update` | `true` | Enable the weekly self-update timer. With `false`, update with `uptime-kinomi self-update`. |

## Example

`site.yml` next to `files/config/`:

```yaml
- name: Deploy uptime-kinomi
  hosts: monitoring
  roles:
    - role: ansible-role-root
      vars:
        KINOMI_USER: kinomi
```

```bash
ANSIBLE_ROLES_PATH=contrib ansible-playbook -i root@pi1.example.org, site.yml
```

For a system-wide service instead of a user service, use
`contrib/systemd/system/`.
